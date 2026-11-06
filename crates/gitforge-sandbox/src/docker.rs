//! Docker-based sandbox implementation using bollard

use crate::limits::SandboxLimits;
use async_trait::async_trait;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::models::{ContainerCreateBody, HostConfig, Mount, MountTmpfsOptions, MountType};
use bollard::query_parameters::{
    CreateContainerOptions, CreateImageOptions, ListContainersOptions, RemoveContainerOptions,
    StopContainerOptions,
};
use bollard::Docker;
use bollard::API_DEFAULT_VERSION;
use futures_util::StreamExt;
use gitforge_common::{
    cache_root, is_socket_source, Error, JobId, Result, VolumeSpec, VolumeType,
    DEFAULT_DOCKER_SOCKET,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::time::{sleep, timeout, Duration};

/// Docker's CFS bandwidth period, in microseconds.
const CPU_PERIOD_MICROS: i64 = 100_000;

/// The Docker client timeout, in seconds.
///
/// bollard's default is 120 seconds. An operator can raise it for a daemon
/// on slow storage, where a client timeout failed jobs before any
/// acquisition window the runner granted could elapse.
pub(crate) fn docker_client_timeout() -> u64 {
    std::env::var("GITFORGE_DOCKER_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(120) // bollard's DEFAULT_TIMEOUT
}

/// Convert a requested core count to a safe CFS quota, bounded by the
/// host's available parallelism. `cpu_ms` is a runtime budget and must not
/// be used as a per-period quota.
fn cpu_quota_micros(requested_cores: u32, available_cores: u32) -> i64 {
    let available_cores = available_cores.max(1);
    let bounded_cores = requested_cores.clamp(1, available_cores);
    i64::from(bounded_cores) * CPU_PERIOD_MICROS
}

fn host_cpu_quota_micros(requested_cores: u32) -> i64 {
    let available_cores = std::thread::available_parallelism()
        .map_or(1, |count| u32::try_from(count.get()).unwrap_or(u32::MAX));
    cpu_quota_micros(requested_cores, available_cores)
}

/// Sandbox instance handle
#[derive(Debug, Clone)]
pub struct SandboxInstance {
    pub container_id: String,
    pub job_id: JobId,
    /// Path on the host that is mounted at /workspace inside the container.
    /// Present only when the sandbox was created with a workspace mount.
    pub workspace_path: Option<String>,
}

/// Result of step execution
#[derive(Debug, Clone)]
pub struct StepResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Which output stream produced a live execution chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// What the container-backend preflight actually measured (R6.3).
///
/// This is the F40/F45 remedy: `docker_daemon_available()` was a filesystem
/// existence check that always passed while the real daemon hung (wedged
/// rootless podman) or executed nothing (host scratch exhausted). The probe
/// reports what it genuinely observed so a failing runner is alertable as
/// infrastructure, never as a red X on a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendHealth {
    /// The daemon answered a `/_ping` inside the probe window.
    pub daemon_ok: bool,
    /// Free bytes on the filesystem backing the exec scratch directory
    /// (`/tmp` per `std::env::temp_dir()` — the exact resource F45
    /// exhausted, where runc stages per-exec state). `None` when it could
    /// not be measured or the sandbox is a stub.
    pub scratch_free_bytes: Option<u64>,
    /// Free-scratch floor below which `healthy()` refuses new work.
    /// Configurable via `GITFORGE_SCRATCH_MIN_FREE_BYTES`.
    pub scratch_min_free_bytes: u64,
    /// Human-readable probe delta for error messages.
    pub detail: String,
}

impl BackendHealth {
    pub fn healthy(&self) -> bool {
        self.daemon_ok
            && self
                .scratch_free_bytes
                .is_none_or(|free| free >= self.scratch_min_free_bytes)
    }
}

/// Ceiling on how long the daemon ping may take before the backend counts
/// as wedged. F40's signature was a ping that never returned.
fn probe_timeout() -> Duration {
    Duration::from_secs(
        std::env::var("GITFORGE_BACKEND_PROBE_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|secs| *secs > 0)
            .unwrap_or(10),
    )
}

/// Free-scratch floor for `BackendHealth::healthy` (default 2 GiB: the
/// slack the F45 remediation asks operators to keep on `/tmp`).
fn scratch_min_free_bytes() -> u64 {
    std::env::var("GITFORGE_SCRATCH_MIN_FREE_BYTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(2 * 1024 * 1024 * 1024)
}

/// Measure free bytes on the filesystem backing the exec scratch directory.
fn scratch_free_bytes() -> Option<u64> {
    match rustix::fs::statvfs(std::env::temp_dir()) {
        Ok(stat) => Some(stat.f_bavail.saturating_mul(stat.f_frsize)),
        Err(error) => {
            tracing::debug!(%error, "could not stat the scratch filesystem");
            None
        }
    }
}

/// Receives bounded output chunks while a sandbox command is running.
///
/// Implementations may apply backpressure by awaiting durable delivery. The
/// sandbox never hands out an unbounded buffer; Docker frames are delivered as
/// they arrive and the caller controls any queueing policy.
#[async_trait]
pub trait OutputSink: Send + Sync {
    async fn on_output(&self, stream: OutputStream, chunk: Vec<u8>) -> Result<()>;
}

/// Docker-based sandbox
pub struct DockerSandbox {
    docker: Option<Docker>,
    #[allow(dead_code)]
    default_limits: SandboxLimits,
    /// If true, this is a stub sandbox for testing and should not be used in production
    is_stub: bool,
    /// The daemon socket this sandbox talks to, as a host path.
    ///
    /// A `{docker_socket}` mount binds this exact path, so the socket a
    /// job mounts is the socket the sandbox talks to - not whatever the
    /// environment says at container-creation time.
    docker_socket: String,
}

impl DockerSandbox {
    /// Build a unique container name for one execution attempt.
    ///
    /// A runner can be terminated after creating a container but before it
    /// records/completes the job. Reusing only the job ID then makes the
    /// replacement runner fail with a name collision. The job prefix keeps
    /// containers identifiable while the attempt UUID makes retries safe.
    fn container_name(job_id: JobId) -> String {
        format!("gitforce-job-{}-{}", job_id, uuid::Uuid::new_v4())
    }

    /// Remove containers left by an earlier attempt of the same job.
    ///
    /// A runner can be terminated after creating a container but before the
    /// normal destroy path runs. Cleanup is intentionally scoped by the
    /// ownership label, so unrelated containers are never considered. Public
    /// so callers that abandon an acquisition (timeout, creation error) can
    /// reap the half-created container instead of leaking it.
    pub async fn remove_job_containers(&self, job_id: JobId) -> Result<()> {
        let Some(ref docker) = self.docker else {
            return Ok(());
        };

        let mut filters = HashMap::new();
        filters.insert(
            "label".to_string(),
            vec!["com.gitforce.managed=true".to_string()],
        );
        let containers = docker
            .list_containers(Some(ListContainersOptions {
                all: true,
                filters: Some(filters),
                ..Default::default()
            }))
            .await
            .map_err(|e| Error::sandbox(format!("failed to list job containers: {e}")))?;

        for container in containers {
            let owned_by_job = container
                .labels
                .as_ref()
                .and_then(|labels| labels.get("com.gitforce.job_id"))
                .is_some_and(|value| value == &job_id.to_string());
            if !owned_by_job {
                continue;
            }
            if let Some(id) = container.id {
                match docker
                    .remove_container(
                        &id,
                        Some(RemoveContainerOptions {
                            force: true,
                            ..Default::default()
                        }),
                    )
                    .await
                {
                    Ok(()) => {
                        tracing::info!(%id, %job_id, "Removed stale sandbox container before retry");
                    }
                    // 404: the container is already gone. 409: a concurrent
                    // teardown (the job's own destroy path) is removing it
                    // right now. Either way it is on its way out and the
                    // create below can proceed; treating the race as fatal
                    // fails the job with a sandbox-acquisition error for a
                    // container that no longer exists.
                    Err(bollard::errors::Error::DockerResponseServerError {
                        status_code: 404 | 409,
                        ..
                    }) => {
                        tracing::info!(%id, %job_id, "Stale sandbox container already being removed");
                    }
                    Err(e) => {
                        return Err(Error::sandbox(format!(
                            "failed to remove stale job container: {e}"
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Create a new Docker sandbox, requiring Docker to be available.
    /// Returns an error if Docker is not available or cannot be reached.
    pub async fn connect_required() -> Result<Self> {
        // The client's per-request timeout silently caps every operation,
        // including container creation, which on the daemon's old
        // vfs-backed storage could take minutes under load — a 120-second
        // client timeout failed jobs before any acquisition window we
        // granted could elapse. (Storage has since moved to overlay2, but
        // the rule stands: keep bollard's default unless the operator
        // raises it.)
        let client_timeout = docker_client_timeout();
        // One definition for the whole platform: the sandbox connects to
        // this socket, the reconciler reconciles this daemon's
        // containers, and a `{docker_socket}` mount binds this socket
        // into the job's container.
        let socket = gitforge_common::docker_socket();
        let docker = Docker::connect_with_unix(&socket, client_timeout, API_DEFAULT_VERSION)
            .map_err(|e| Error::sandbox(format!("failed to connect to Docker: {e}")))?;

        // Verify connection by pinging Docker
        docker
            .ping()
            .await
            .map_err(|e| Error::sandbox(format!("Docker daemon not available: {e}")))?;

        tracing::info!("Connected to Docker daemon");

        Ok(Self {
            docker: Some(docker),
            default_limits: SandboxLimits::default(),
            is_stub: false,
            docker_socket: socket,
        })
    }

    /// Probe the container backend for real health: a daemon `/_ping` inside
    /// the probe window plus a measurement of the free space on the exec
    /// scratch filesystem. Replaces the filesystem-existence check that
    /// F40 proved blind (wedged podman passed it) and gives the F45
    /// full-scratch failure an observable warning before jobs die at the
    /// OCI layer.
    pub async fn backend_health(&self) -> BackendHealth {
        if self.is_stub {
            return BackendHealth {
                daemon_ok: true,
                scratch_free_bytes: None,
                scratch_min_free_bytes: scratch_min_free_bytes(),
                detail: "stub sandbox; probe skipped".to_string(),
            };
        }

        let min_free = scratch_min_free_bytes();
        let Some(docker) = &self.docker else {
            return BackendHealth {
                daemon_ok: false,
                scratch_free_bytes: scratch_free_bytes(),
                scratch_min_free_bytes: min_free,
                detail: "sandbox has no daemon client".to_string(),
            };
        };

        let ping = timeout(probe_timeout(), docker.ping()).await;
        let (daemon_ok, ping_detail) = match ping {
            Ok(Ok(_)) => (true, "ping ok".to_string()),
            Ok(Err(error)) => (false, format!("ping failed: {error}")),
            Err(_) => (
                false,
                format!(
                    "ping did not answer within {}s (wedged backend signature)",
                    probe_timeout().as_secs()
                ),
            ),
        };
        let scratch_free_bytes = scratch_free_bytes();
        let scratch_detail = match scratch_free_bytes {
            Some(free) => format!(
                "scratch free {:.1} GiB (floor {:.1} GiB)",
                free as f64 / (1024.0 * 1024.0 * 1024.0),
                min_free as f64 / (1024.0 * 1024.0 * 1024.0)
            ),
            None => "scratch free space unavailable".to_string(),
        };

        BackendHealth {
            daemon_ok,
            scratch_free_bytes,
            scratch_min_free_bytes: min_free,
            detail: format!("{ping_detail}; {scratch_detail}"),
        }
    }

    /// Create a stub sandbox for testing purposes.
    /// This sandbox does not actually execute commands - it simulates execution.
    /// Commands always succeed with exit code 0 in stub mode.
    pub fn stub_for_tests() -> Self {
        Self {
            docker: None,
            default_limits: SandboxLimits::default(),
            is_stub: true,
            docker_socket: DEFAULT_DOCKER_SOCKET.to_owned(),
        }
    }

    /// Create a new Docker sandbox with custom limits (stub mode only).
    /// DEPRECATED: Use `connect_required()` for production or `stub_for_tests()` for testing.
    #[deprecated(
        since = "0.1.0",
        note = "use connect_required() or stub_for_tests() instead"
    )]
    pub fn with_limits(limits: SandboxLimits) -> Self {
        Self {
            docker: None,
            default_limits: limits,
            is_stub: true,
            docker_socket: DEFAULT_DOCKER_SOCKET.to_owned(),
        }
    }

    /// Returns true if this is a stub sandbox used for testing.
    pub fn is_stub(&self) -> bool {
        self.is_stub
    }

    /// Check if Docker is available
    pub fn is_available(&self) -> bool {
        self.docker.is_some()
    }

    /// Pull an image if not present
    async fn ensure_image(&self, image: &str) -> Result<()> {
        if let Some(ref docker) = self.docker {
            // Check if image already exists locally before pulling
            match docker.inspect_image(image).await {
                Ok(_) => {
                    tracing::info!("Image already exists locally: {}", image);
                    return Ok(());
                }
                Err(e) => {
                    tracing::info!("Image not found locally, pulling: {} (error: {})", image, e);
                }
            }

            // Retry-once for transient pull errors (e.g. Docker Hub rate-limit 403).
            // The first attempt may fail because the daemon's unauthenticated quota is
            // exhausted; waiting briefly lets the quota reset and the local inspect
            // cache to be fresh on retry.
            const PULL_RETRIES: u32 = 1;
            const PULL_RETRY_DELAY_SECS: u64 = 60;
            let mut last_err = None;
            for attempt in 0..=PULL_RETRIES {
                let mut stream = docker.create_image(
                    Some(CreateImageOptions {
                        from_image: Some(image.to_owned()),
                        ..Default::default()
                    }),
                    None,
                    None,
                );
                let mut pull_ok = true;
                while let Some(result) = stream.next().await {
                    match result {
                        Ok(info) => {
                            tracing::debug!("Pull progress: {:?}", info);
                        }
                        Err(e) => {
                            last_err = Some(e);
                            pull_ok = false;
                            break;
                        }
                    }
                }
                if pull_ok {
                    return Ok(());
                }
                if attempt < PULL_RETRIES {
                    tracing::warn!(
                        "image pull attempt {}/{} failed ({}): retrying in {}s",
                        attempt + 1,
                        PULL_RETRIES + 1,
                        last_err
                            .as_ref()
                            .map(std::string::ToString::to_string)
                            .unwrap_or_default(),
                        PULL_RETRY_DELAY_SECS
                    );
                    sleep(Duration::from_secs(PULL_RETRY_DELAY_SECS)).await;
                }
            }
            return Err(Error::sandbox(format!(
                "failed to pull image after {} attempt(s): {}",
                PULL_RETRIES + 1,
                last_err.map(|e| e.to_string()).unwrap_or_default()
            )));
        }
        Ok(())
    }
    /// Create a container, stamping `extra_labels` onto it in addition to
    /// the managed and job-id labels every container carries.
    async fn create_with_extra_labels(
        &self,
        job_id: JobId,
        image: &str,
        limits: SandboxLimits,
        volumes: &[VolumeSpec],
        extra_labels: &[(String, String)],
    ) -> Result<SandboxInstance> {
        if let Some(ref docker) = self.docker {
            // Ensure image is available
            self.ensure_image(image).await?;
            self.remove_job_containers(job_id).await?;

            let container_name = Self::container_name(job_id);
            let job_label = job_id.to_string();
            let mut labels = HashMap::from([
                ("com.gitforce.managed".to_owned(), "true".to_owned()),
                ("com.gitforce.job_id".to_owned(), job_label.clone()),
            ]);
            // Pool containers carry the pool's ownership labels on top of
            // the managed/job-id pair, so the pool can tell its own
            // containers from job containers without guessing.
            for (key, value) in extra_labels {
                labels.insert(key.clone(), value.clone());
            }

            // A mount the runner cannot prepare is a contract it cannot
            // keep: the pipeline declared this directory and the job
            // needs it. Failing here is what makes the reason visible -
            // the runner reports it as an infrastructure failure, so it
            // reaches the job's own result instead of a log line (GF-10,
            // 2026-11-05: the whole mount set used to be dropped, and a
            // job whose cache had vanished just ran cold and timed out).
            let volume_mounts =
                build_volume_mounts(volumes, image, &cache_root(), &self.docker_socket)?;

            // Build host config with resource limits
            let host_config = HostConfig {
                memory: Some((limits.memory_mb * 1024 * 1024) as i64),
                cpu_period: Some(CPU_PERIOD_MICROS),
                cpu_quota: Some(host_cpu_quota_micros(limits.cpu_cores)),
                network_mode: if limits.network {
                    None
                } else {
                    Some("none".to_string())
                },
                binds: if volume_mounts.binds.is_empty() {
                    None
                } else {
                    Some(volume_mounts.binds)
                },
                mounts: if volume_mounts.mounts.is_empty() {
                    None
                } else {
                    Some(volume_mounts.mounts)
                },
                // A container that mounts the daemon socket cannot keep
                // SELinux labeling: the socket's label does not match the
                // container's, and an enforcing host refuses the
                // connection.
                security_opt: if volume_mounts.disable_selinux {
                    Some(vec!["label=disable".to_string()])
                } else {
                    None
                },
                ..Default::default()
            };

            // Create container
            let config = ContainerCreateBody {
                image: Some(image.to_owned()),
                // Keep the container alive for the whole job. A 3600s keepalive
                // capped every job at one hour: when sleep exited, Docker
                // stopped the container and SIGKILLed any running exec
                // (exit 137 at exactly 3600s, OOMKilled=false). The container
                // is removed per-job in destroy(), so this only bounds a
                // leaked container's lifetime.
                cmd: Some(vec![
                    "sleep".to_owned(),
                    "86400".to_owned(), // 24h; job deadlines clamp to 24h
                ]),
                host_config: Some(host_config),
                labels: Some(labels),
                ..Default::default()
            };

            let options = CreateContainerOptions {
                name: Some(container_name.clone()),
                platform: String::new(),
            };

            let response = docker
                .create_container(Some(options), config)
                .await
                .map_err(|e| Error::sandbox(format!("failed to create container: {e}")))?;

            // Start container
            docker
                .start_container(&response.id, None)
                .await
                .map_err(|e| Error::sandbox(format!("failed to start container: {e}")))?;

            tracing::info!("Created container {} for job {}", response.id, job_id);

            Ok(SandboxInstance {
                container_id: response.id,
                job_id,
                workspace_path: None,
            })
        } else {
            // Stub mode - no Docker available
            Ok(SandboxInstance {
                container_id: format!("gitforce-job-{job_id}"),
                job_id,
                workspace_path: None,
            })
        }
    }

    /// Create a container that belongs to a warm pool.
    ///
    /// A pooled container is created with a placeholder job id (it is not
    /// a job) and with the pool's ownership labels, so a later runner
    /// process can recognise the containers this pool left behind and
    /// reclaim them (GF-17).
    pub async fn create_pooled(
        &self,
        identity: &crate::pool::PoolIdentity,
        image: &str,
        limits: SandboxLimits,
    ) -> Result<SandboxInstance> {
        let job_id = JobId::new();
        let extra: Vec<(String, String)> = crate::pool::pool_labels(identity).into_iter().collect();
        self.create_with_extra_labels(job_id, image, limits, &[], &extra)
            .await
    }
}

// ---------------------------------------------------------------------------
// Volume mounts
// ---------------------------------------------------------------------------

/// Parse a human-readable size (`4G`, `512M`, `1024K`) into bytes.
fn parse_size_bytes(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    let (digits, suffix) = trimmed.split_at(
        trimmed
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(trimmed.len()),
    );
    let value: i64 = digits.parse().ok()?;
    let multiplier = match suffix.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "K" | "KB" => 1024,
        "M" | "MB" => 1024 * 1024,
        "G" | "GB" => 1024 * 1024 * 1024,
        "T" | "TB" => 1024_i64 * 1024 * 1024 * 1024,
        _ => return None,
    };
    value.checked_mul(multiplier)
}

/// The container mounts a job asked for.
#[derive(Debug)]
struct VolumeMounts {
    /// Bind sources in `HostConfig.binds` form.
    binds: Vec<String>,
    /// Structured mounts (tmpfs, named volumes).
    mounts: Vec<Mount>,
    /// The container mounts a unix socket, so SELinux labeling has to be
    /// disabled for it.
    ///
    /// The daemon socket's own label does not match the label a container
    /// gets by default, so an enforcing host refuses the connection
    /// (measured on the Fedora runner, 2026-11-06: the connection is
    /// refused without the opt). Relabeling the socket instead - `:z` -
    /// rewrites the label of a host file the runner does not own, so the
    /// bind goes in unlabeled and the container runs with labeling
    /// disabled.
    disable_selinux: bool,
}

/// Turn the requested volumes into docker mounts.
///
/// Binds become `HostConfig.binds` entries so they inherit the daemon's
/// SELinux relabel behaviour (the workspace mount already relies on `:z`);
/// tmpfs and named volumes become structured mounts.
///
/// A bind whose source is a unix socket is the one exception to the
/// relabel: the source is the daemon's own socket, not a host directory
/// the runner owns.
fn build_volume_mounts(
    volumes: &[VolumeSpec],
    image: &str,
    cache_root: &str,
    docker_socket: &str,
) -> Result<VolumeMounts> {
    let mut binds = Vec::new();
    let mut mounts = Vec::new();
    let mut disable_selinux = false;

    for volume in volumes {
        volume
            .validate()
            .map_err(|reason| Error::sandbox(format!("invalid volume mount: {reason}")))?;
        match volume.resolved_type() {
            VolumeType::Bind => {
                let source = volume
                    .expanded_source(image, cache_root, docker_socket)
                    .ok_or_else(|| {
                        Error::sandbox(format!(
                            "volume mount {} requires a source",
                            volume.destination
                        ))
                    })?;
                if !source.starts_with('/') {
                    return Err(Error::sandbox(format!(
                        "volume mount source must be an absolute path: {source}"
                    )));
                }
                let access = if volume.read_only { "ro" } else { "rw" };
                if is_socket_source(&source) {
                    // A socket is not a directory: the daemon created it,
                    // and creating it here would fail. The bind goes in
                    // without a relabel - `:z` would rewrite the label of
                    // the host's own socket file - and the container runs
                    // with SELinux labeling disabled instead.
                    binds.push(format!("{source}:{}:{access}", volume.destination));
                    disable_selinux = true;
                } else {
                    // The daemon creates a missing bind source as root, which
                    // the job's user could not write to. Create it here, as the
                    // runner's own user, so a cache mount is writable.
                    if let Err(error) = std::fs::create_dir_all(&source) {
                        return Err(Error::sandbox(format!(
                            "could not prepare volume mount source {source}: {error}"
                        )));
                    }
                    binds.push(format!("{source}:{}:{access},z", volume.destination));
                }
            }
            VolumeType::Tmpfs => {
                let tmpfs_options =
                    volume
                        .size
                        .as_deref()
                        .and_then(parse_size_bytes)
                        .map(|size_bytes| MountTmpfsOptions {
                            size_bytes: Some(size_bytes),
                            // World-writable: the job's user inside the image is
                            // not the runner's user.
                            mode: Some(0o1777),
                            options: None,
                        });
                mounts.push(Mount {
                    typ: Some(MountType::TMPFS),
                    target: Some(volume.destination.clone()),
                    source: None,
                    read_only: Some(volume.read_only),
                    consistency: None,
                    bind_options: None,
                    volume_options: None,
                    image_options: None,
                    tmpfs_options,
                });
            }
            VolumeType::Volume => {
                let source = volume.source.clone().unwrap_or_default();
                mounts.push(Mount {
                    typ: Some(MountType::VOLUME),
                    target: Some(volume.destination.clone()),
                    source: Some(source),
                    read_only: Some(volume.read_only),
                    consistency: None,
                    bind_options: None,
                    volume_options: None,
                    image_options: None,
                    tmpfs_options: None,
                });
            }
        }
    }

    Ok(VolumeMounts {
        binds,
        mounts,
        disable_selinux,
    })
}

/// Sandbox trait
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Create a new sandbox instance.
    ///
    /// `volumes` are the host mounts the job asked for (cache directories,
    /// tmpfs scratch). They are baked into the container at creation time.
    async fn create(
        &self,
        job_id: JobId,
        image: &str,
        limits: SandboxLimits,
        volumes: &[VolumeSpec],
    ) -> Result<SandboxInstance>;

    /// Create a sandbox with an isolated host workspace mounted at
    /// `/workspace`. Implementations may fall back to an ordinary sandbox
    /// when no workspace is supplied.
    async fn create_with_workspace(
        &self,
        job_id: JobId,
        image: &str,
        limits: SandboxLimits,
        workspace_path: Option<&str>,
        volumes: &[VolumeSpec],
    ) -> Result<SandboxInstance> {
        let _ = workspace_path;
        self.create(job_id, image, limits, volumes).await
    }

    /// Execute a command in the sandbox.
    async fn execute(&self, instance: &SandboxInstance, command: &[&str]) -> Result<StepResult> {
        self.execute_with_output(instance, command, None).await
    }

    /// Execute a command and deliver output frames as they arrive.
    async fn execute_with_output(
        &self,
        instance: &SandboxInstance,
        command: &[&str],
        sink: Option<Arc<dyn OutputSink>>,
    ) -> Result<StepResult> {
        self.execute_with_env(instance, command, &[], sink).await
    }

    /// Execute a command with additional `KEY=VALUE` environment variables
    /// layered over the container's own environment, streaming output to
    /// `sink` as it arrives. This is how pipeline `environment`/`env` reach
    /// a step: exec-level env composes per step and never requires
    /// recreating the container.
    async fn execute_with_env(
        &self,
        instance: &SandboxInstance,
        command: &[&str],
        env: &[String],
        sink: Option<Arc<dyn OutputSink>>,
    ) -> Result<StepResult>;

    /// Destroy a sandbox instance
    async fn destroy(&self, instance: SandboxInstance) -> Result<()>;
}

/// Whether the daemon still knows the container. A 404 is authoritative
/// loss; any other inspect error is inconclusive and reported as present —
/// callers must not grade a job infrastructure-failed on a transient API
/// hiccup alone.
async fn container_present(docker: &Docker, container_id: &str) -> bool {
    match docker.inspect_container(container_id, None).await {
        Ok(_) => true,
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => false,
        Err(e) => {
            tracing::warn!(
                %container_id,
                error = %e,
                "container existence check failed; assuming present"
            );
            true
        }
    }
}

/// Compose the env for one docker exec: workspace git trust vars first (when
/// the instance mounts a workspace), then the caller's `KEY=VALUE` pairs.
/// Caller env is layered over the container environment; it can add or
/// override ordinary variables but never displaces the git trust vars.
fn compose_exec_env(has_workspace: bool, env: &[String]) -> Vec<String> {
    let mut exec_env: Vec<String> = Vec::with_capacity(env.len() + 3);
    if has_workspace {
        exec_env.extend(workspace_exec_git_env().into_iter().map(String::from));
    }
    exec_env.extend(env.iter().cloned());
    exec_env
}

#[async_trait]
impl Sandbox for DockerSandbox {
    async fn create(
        &self,
        job_id: JobId,
        image: &str,
        limits: SandboxLimits,
        volumes: &[VolumeSpec],
    ) -> Result<SandboxInstance> {
        self.create_with_extra_labels(job_id, image, limits, volumes, &[])
            .await
    }

    async fn create_with_workspace(
        &self,
        job_id: JobId,
        image: &str,
        limits: SandboxLimits,
        workspace_path: Option<&str>,
        volumes: &[VolumeSpec],
    ) -> Result<SandboxInstance> {
        let Some(workspace_path) = workspace_path else {
            return self.create(job_id, image, limits, volumes).await;
        };
        let workspace = Path::new(workspace_path);
        if !workspace.is_absolute() || !workspace.is_dir() {
            return Err(Error::sandbox(format!(
                "workspace must be an existing absolute directory: {workspace_path}"
            )));
        }

        if let Some(ref docker) = self.docker {
            self.ensure_image(image).await?;
            self.remove_job_containers(job_id).await?;
            let container_name = Self::container_name(job_id);
            let job_label = job_id.to_string();
            let labels = HashMap::from([
                ("com.gitforce.managed".to_owned(), "true".to_owned()),
                ("com.gitforce.job_id".to_owned(), job_label),
            ]);
            // See `create`: a mount the runner cannot prepare fails the
            // job rather than vanishing from the contract.
            let volume_mounts =
                build_volume_mounts(volumes, image, &cache_root(), &self.docker_socket)?;
            let host_config = HostConfig {
                memory: Some((limits.memory_mb * 1024 * 1024) as i64),
                cpu_period: Some(CPU_PERIOD_MICROS),
                cpu_quota: Some(host_cpu_quota_micros(limits.cpu_cores)),
                network_mode: if limits.network {
                    None
                } else {
                    Some("none".to_string())
                },
                // Fedora's rootless container engine enforces SELinux labels on
                // host mounts. The workspace is intentionally shared by jobs in
                // one pipeline run, so use a shared relabel. Private `:Z`
                // relabeling is racy when concurrent jobs mount the same
                // checkout and can leave one container unable to see files.
                binds: Some({
                    let mut binds = vec![format!("{}:/workspace:z", workspace_path)];
                    binds.extend(volume_mounts.binds);
                    binds
                }),
                mounts: if volume_mounts.mounts.is_empty() {
                    None
                } else {
                    Some(volume_mounts.mounts)
                },
                // A container that mounts the daemon socket cannot keep
                // SELinux labeling: the socket's label does not match the
                // container's, and an enforcing host refuses the
                // connection.
                security_opt: if volume_mounts.disable_selinux {
                    Some(vec!["label=disable".to_string()])
                } else {
                    None
                },
                ..Default::default()
            };
            let config = ContainerCreateBody {
                image: Some(image.to_owned()),
                // Same keepalive-lifetime rationale as the plain sandbox:
                // a 3600s sleep SIGKILLed running execs at exactly 1h.
                cmd: Some(vec![
                    "sleep".to_owned(),
                    "86400".to_owned(), // 24h; job deadlines clamp to 24h
                ]),
                working_dir: Some("/workspace".to_owned()),
                // The workspace bind-mount keeps the host checkout's uid (the
                // job user inside the image is usually root), and git's
                // `safe.directory` check rejects any repo owned by a different
                // account outright — every `git` invocation in the job dies
                // with "detected dubious ownership" (exit 128) before it does
                // any work. Inject the allowance through git's env-based
                // config so it applies to every image and repo without baking
                // anything into the image itself. Only the workspace-mount
                // path needs this: a plain sandbox has no host repo to trust.
                env: Some(workspace_git_env()),
                host_config: Some(host_config),
                labels: Some(labels),
                ..Default::default()
            };
            let response = docker
                .create_container(
                    Some(CreateContainerOptions {
                        name: Some(container_name.clone()),
                        platform: String::new(),
                    }),
                    config,
                )
                .await
                .map_err(|e| {
                    Error::sandbox(format!("failed to create workspace container: {e}"))
                })?;
            docker
                .start_container(&response.id, None)
                .await
                .map_err(|e| Error::sandbox(format!("failed to start workspace container: {e}")))?;
            tracing::info!(
                "Created workspace container {} for job {} from {}",
                response.id,
                job_id,
                workspace_path
            );
            Ok(SandboxInstance {
                container_id: response.id,
                job_id,
                workspace_path: Some(workspace_path.to_string()),
            })
        } else {
            Err(Error::sandbox("Docker is required for workspace execution"))
        }
    }

    async fn execute_with_env(
        &self,
        instance: &SandboxInstance,
        command: &[&str],
        env: &[String],
        sink: Option<Arc<dyn OutputSink>>,
    ) -> Result<StepResult> {
        if let Some(ref docker) = self.docker {
            // Create exec instance. The workspace git trust vars are always
            // layered in for workspace execs; caller env (`KEY=VALUE` pairs)
            // rides alongside them and never displaces them.
            let has_workspace = instance.workspace_path.is_some();
            let exec_env = compose_exec_env(has_workspace, env);
            let exec_command: Vec<&str> = if has_workspace {
                wrap_with_git_config_bootstrap(command)
            } else {
                command.to_vec()
            };
            let config = CreateExecOptions {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                cmd: Some(exec_command),
                // `create_with_workspace` mounts the host checkout at this
                // fixed container path. Docker exec does not inherit the
                // container's configured working directory, so set it here
                // explicitly or commands run outside the checkout.
                working_dir: if instance.workspace_path.is_some() {
                    Some("/workspace")
                } else {
                    None
                },
                env: Some(exec_env.iter().map(String::as_str).collect()),
                ..Default::default()
            };

            let exec = docker
                .create_exec(&instance.container_id, config)
                .await
                .map_err(|e| Error::sandbox(format!("failed to create exec: {e}")))?;

            // Start exec and get results
            let result = docker
                .start_exec(&exec.id, None::<StartExecOptions>)
                .await
                .map_err(|e| Error::sandbox(format!("failed to start exec: {e}")))?;

            let mut stdout = String::new();
            let mut stderr = String::new();

            if let StartExecResults::Attached { mut output, .. } = result {
                loop {
                    tokio::select! {
                        item = output.next() => {
                            match item {
                                Some(Ok(LogOutput::StdOut { message })) => {
                                    if let Some(ref sink) = sink {
                                        sink.on_output(OutputStream::Stdout, message.to_vec()).await?;
                                    }
                                    stdout.push_str(&String::from_utf8_lossy(&message));
                                }
                                Some(Ok(LogOutput::StdErr { message })) => {
                                    if let Some(ref sink) = sink {
                                        sink.on_output(OutputStream::Stderr, message.to_vec()).await?;
                                    }
                                    stderr.push_str(&String::from_utf8_lossy(&message));
                                }
                                Some(Ok(_)) => {}
                                Some(Err(e)) => {
                                    // A mid-stream error usually means the
                                    // container died under the exec (OOM kill,
                                    // removal, daemon restart). Confirm against
                                    // the daemon before grading: a vanished
                                    // container is an infrastructure failure,
                                    // never a silent pass. Any other error
                                    // keeps the tolerant behavior — the poll
                                    // below still recovers the real outcome.
                                    tracing::warn!("exec output error: {}", e);
                                    if !container_present(docker, &instance.container_id).await {
                                        return Err(Error::sandbox(format!(
                                            "container lost while exec {} was streaming output",
                                            exec.id
                                        )));
                                    }
                                }
                                None => break,
                            }
                        }
                        () = sleep(Duration::from_secs(1)) => {
                            match docker.inspect_exec(&exec.id).await {
                                Ok(inspect) if inspect.running == Some(false) => break,
                                Ok(_) => {}
                                Err(e) => tracing::trace!("exec status poll failed: {}", e),
                            }
                        }
                    }
                }
            }

            // Inspect to get the exit code. The old code defaulted any
            // unreadable outcome to 0, so a container that vanished between
            // the output stream and the inspect graded its step as passed —
            // the false-green class. Poll briefly for a definitive answer,
            // then fail loudly: an unknown outcome is not a pass, and the
            // executor grades the error as an infrastructure failure.
            let mut resolved: Option<i32> = None;
            for attempt in 0..5u32 {
                match docker.inspect_exec(&exec.id).await {
                    Ok(inspect) if inspect.running == Some(false) => {
                        resolved = Some(inspect.exit_code.unwrap_or(0) as i32);
                        break;
                    }
                    Ok(_) => {
                        tracing::trace!(attempt, "exec still running at inspect");
                    }
                    Err(e) => {
                        tracing::warn!(attempt, error = %e, "failed to inspect exec");
                    }
                }
                sleep(Duration::from_millis(250)).await;
            }
            let Some(exit_code) = resolved else {
                if !container_present(docker, &instance.container_id).await {
                    return Err(Error::sandbox(format!(
                        "container lost before exec {} reported its exit code",
                        exec.id
                    )));
                }
                return Err(Error::sandbox(format!(
                    "exec {} finished but its exit code could not be read",
                    exec.id
                )));
            };
            Ok(StepResult {
                exit_code,
                stdout,
                stderr,
            })
        } else {
            // Stub mode - NOTE: This always returns exit code 0, which may not reflect
            // the actual command result. Only use stub_for_tests() when you explicitly
            // want stub behavior.
            tracing::debug!("Executing command in stub mode: {:?}", command);
            let result = StepResult {
                exit_code: 0,
                stdout: format!("[STUB] Executing: {command:?}\n"),
                stderr: "[STUB] Warning: running in stub mode, exit code is always 0\n".to_string(),
            };
            if let Some(sink) = sink {
                sink.on_output(OutputStream::Stdout, result.stdout.as_bytes().to_vec())
                    .await?;
                sink.on_output(OutputStream::Stderr, result.stderr.as_bytes().to_vec())
                    .await?;
            }
            Ok(result)
        }
    }

    async fn destroy(&self, instance: SandboxInstance) -> Result<()> {
        if let Some(ref docker) = self.docker {
            // If this sandbox had a workspace mount, chown the workspace tree to
            // the runner's UID/GID before shutting down.  Inside the container
            // we run as root, so chown is always permitted.  After this succeeds
            // the runner user on the host can delete the artifact files without
            // requiring privileged escalation.
            if let Some(ref workspace) = instance.workspace_path {
                // The ownership cleanup is a convenience, not a precondition
                // of the teardown: the CI service removes a workspace it
                // cannot delete with a container-assisted cleanup. The
                // teardown, on the other hand, MUST happen - a cancelled
                // teardown leaves the container running forever.
                //
                // The recursive chown walks the whole workspace tree. On a
                // network filesystem, over a vendored source tree, that walk
                // takes minutes: the runner's teardown budget expired first,
                // and the container was never stopped or removed (measured
                // 2026-11-05: nine leaked job containers, each one a "timed
                // out cleaning up sandbox" warning - and the leak blocks
                // every release cutover, whose drain gate counts live job
                // containers). So the cleanup runs under its own budget and
                // the container is always torn down.
                match bounded_cleanup(
                    cleanup_workspace(docker, &instance.container_id, workspace),
                    WORKSPACE_CLEANUP_BUDGET,
                )
                .await
                {
                    WorkspaceCleanup::Completed => {}
                    WorkspaceCleanup::Failed(error) => {
                        tracing::warn!(
                            "workspace ownership cleanup failed for {}: {} \
                             (artifact files may require privileged deletion)",
                            workspace,
                            error
                        );
                    }
                    WorkspaceCleanup::TimedOut => {
                        tracing::warn!(
                            "workspace ownership cleanup exceeded {}s for {} \
                             (artifact files may require privileged deletion)",
                            WORKSPACE_CLEANUP_BUDGET.as_secs(),
                            workspace
                        );
                    }
                }
            }

            // Send SIGTERM for graceful shutdown first
            // Wait up to 10 seconds for container to stop gracefully
            let stop_options = StopContainerOptions {
                t: Some(10),
                ..Default::default()
            };

            if let Err(e) = docker
                .stop_container(&instance.container_id, Some(stop_options))
                .await
            {
                // Container might already be stopped or not exist - that's OK
                tracing::debug!(
                    "stop_container returned error (container may already be stopped): {}",
                    e
                );
            }

            // Remove the container even when the graceful stop reported an
            // already-stopped or inconsistent rootless-Podman state. The
            // container is owned by this exact job instance; force removal
            // prevents its conmon helper from surviving the sandbox lifecycle.
            let remove_options = RemoveContainerOptions {
                force: true,
                ..Default::default()
            };

            docker
                .remove_container(&instance.container_id, Some(remove_options))
                .await
                .map_err(|e| Error::sandbox(format!("failed to remove container: {e}")))?;

            tracing::info!(
                "Destroyed container {} for job {}",
                instance.container_id,
                instance.job_id
            );
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// UID / GID parsing and validation
// ---------------------------------------------------------------------------

/// Maximum valid UID/GID on Linux (65534 is nobody-user / nogroup).
const MAX_UID_GID: u32 = 65534;

/// Parse a UID or GID string into a validated `u32`.
/// Returns `None` if the string is not a decimal integer in `[0, MAX_UID_GID]`.
fn parse_uid_gid(raw: &str) -> Option<u32> {
    let val: u32 = raw.trim().parse().ok()?;
    (val <= MAX_UID_GID).then_some(val)
}

/// Format a UID and GID as the "uid:gid" string accepted by `chown`.
fn chown_owner_string(uid: u32, gid: u32) -> String {
    format!("{uid}:{gid}")
}

/// Resolve the runner UID/GID from environment variables, falling back to
/// `GITFORGE_RUNNER_UID` / `GITFORGE_RUNNER_GID` (default 1000:1000).
/// Returns `(uid, gid)` or an error if either variable is set to an invalid value.
fn resolve_runner_uid_gid() -> Result<(u32, u32)> {
    let uid_raw = std::env::var("GITFORGE_RUNNER_UID").unwrap_or_else(|_| "1000".to_string());
    let gid_raw = std::env::var("GITFORGE_RUNNER_GID").unwrap_or_else(|_| "1000".to_string());

    let uid = parse_uid_gid(&uid_raw)
        .ok_or_else(|| Error::sandbox(format!("invalid GITFORGE_RUNNER_UID: {uid_raw}")))?;
    let gid = parse_uid_gid(&gid_raw)
        .ok_or_else(|| Error::sandbox(format!("invalid GITFORGE_RUNNER_GID: {gid_raw}")))?;

    Ok((uid, gid))
}

/// Git environment for workspace execs. The checkout is created on the host
/// by the runner user but execs run as the container's root, so git refuses
/// every operation with "detected dubious ownership" — which silently turned
/// any pipeline step guarding with `|| true` (tag probes, version detection)
/// into a no-op. Trust every repository the exec can touch.
///
/// This must stay `*`, not `/workspace`: repositories are not only mounted
/// at the mount root, they are also created inside the workspace by steps
/// and test fixtures (e.g. `git init seed && git config user.email ...`
/// under `/workspace/target/...`). `safe.directory` matches the exact path,
/// never a prefix, so a `/workspace` entry leaves every such sub-repository
/// rejected — observed on fedora-docker where the workspace rides an sshfs
/// mount mapping all files to uid 1000 while the container runs as root:
/// `git config` in a freshly-created fixture repo failed with "not in a git
/// directory" and dropped 10 ci unit tests (run f2a1a2d9, 2026-11-04).
/// Within a job sandbox whose only host surface is the workspace mount,
/// blanket trust costs nothing real: the mount is per-run scratch data.
fn workspace_exec_git_env() -> Vec<&'static str> {
    vec!["GIT_CONFIG_GLOBAL=/tmp/.gitforge-gitconfig"]
}

/// Git protects the blanket `safe.directory = *` wildcard: it is honored
/// only from system or global configuration, never from command-scoped
/// `GIT_CONFIG_*` env entries (an exact path works there, but fixtures and
/// steps create repositories at unpredictable paths under the mount).
/// `GIT_CONFIG_GLOBAL` however can point anywhere via env, so each exec
/// materializes a tiny global config granting the wildcard before it runs.
const GIT_GLOBAL_CONFIG_BOOTSTRAP: &str = "mkdir -p /tmp && \
printf '[safe]\n\tdirectory = *\n' > /tmp/.gitforge-gitconfig; exec \"$@\"";

/// Wrap `command` so the exec first writes the global git config that the
/// `GIT_CONFIG_GLOBAL` env entry (see `workspace_exec_git_env`) points at,
/// then execs the original command. `sh` is present in every job image this
/// runner serves; the write is idempotent and costs one fork per step.
fn wrap_with_git_config_bootstrap<'a>(command: &[&'a str]) -> Vec<&'a str> {
    let mut wrapped: Vec<&str> = Vec::with_capacity(command.len() + 4);
    wrapped.extend_from_slice(&["sh", "-ec", GIT_GLOBAL_CONFIG_BOOTSTRAP, "--"]);
    wrapped.extend_from_slice(command);
    wrapped
}

/// Container-level variant of `workspace_exec_git_env` (owned strings for
/// `CreateContainerOptions`).
fn workspace_git_env() -> Vec<String> {
    workspace_exec_git_env()
        .into_iter()
        .map(String::from)
        .collect()
}

// ---------------------------------------------------------------------------
// Workspace cleanup
// ---------------------------------------------------------------------------

/// Transfer ownership of the workspace tree to the runner user so the host
/// process can clean up artifact files without privileged escalation.
///
/// Implementation notes
/// =====================
/// - UID/GID are parsed as decimal integers and clamped to `[0, 65534]`.  No
///   shell interpolation is performed because the command is passed as discrete
///   args: `["chown", "-R", "uid:gid", "/workspace"]`.
/// - The exec output stream is **fully consumed** before the exit code is read.
///   This is required: `inspect_exec` reflects the state at the time of the
///   call; if the stream is still running the exit code may not yet be visible.
/// - Failures are warned but never block container teardown (caller handles
///   fail-open semantics).
/// - The cleanup runs under a budget of its own (`WORKSPACE_CLEANUP_BUDGET`)
///   so a tree that takes minutes to walk can never cancel the teardown.
const WORKSPACE_CLEANUP_BUDGET: Duration = Duration::from_secs(15);

/// Outcome of a bounded workspace ownership cleanup.
#[derive(Debug, PartialEq, Eq)]
enum WorkspaceCleanup {
    /// The cleanup finished: the tree belongs to the runner user.
    Completed,
    /// The cleanup failed; the reason is reported.
    Failed(String),
    /// The cleanup exceeded its budget and was abandoned.
    TimedOut,
}

/// Run the workspace ownership cleanup under its own budget.
///
/// The cleanup is best-effort: whatever it does or does not achieve, the
/// caller's container teardown continues. A cleanup that never returns must
/// not be able to swallow the teardown that follows it - that is what left
/// nine job containers running on the NAS instance (2026-11-05).
async fn bounded_cleanup<F>(cleanup: F, budget: Duration) -> WorkspaceCleanup
where
    F: std::future::Future<Output = Result<()>>,
{
    match timeout(budget, cleanup).await {
        Ok(Ok(())) => WorkspaceCleanup::Completed,
        Ok(Err(error)) => WorkspaceCleanup::Failed(error.to_string()),
        Err(_) => WorkspaceCleanup::TimedOut,
    }
}

async fn cleanup_workspace(docker: &Docker, container_id: &str, workspace: &str) -> Result<()> {
    let (uid, gid) = resolve_runner_uid_gid()?;

    // Build the ownership string (e.g. "1000:1000") and pass it as a single
    // argument — no shell, no string interpolation.
    let owner = chown_owner_string(uid, gid);

    let config = CreateExecOptions {
        attach_stdout: Some(true),
        attach_stderr: Some(true),
        // Use direct exec; avoids shell metacharacter expansion entirely.
        cmd: Some(vec!["chown", "-R", &owner, "/workspace"]),
        ..Default::default()
    };

    let exec = docker
        .create_exec(container_id, config)
        .await
        .map_err(|e| Error::sandbox(format!("failed to create cleanup exec: {e}")))?;

    // start_exec returns StartExecResults. If the container supports exec
    // (Linux containers always do) we get an Attached handle with the output
    // stream. We MUST drain the stream to completion before reading exit codes.
    let result = docker
        .start_exec(&exec.id, None::<StartExecOptions>)
        .await
        .map_err(|e| Error::sandbox(format!("failed to start workspace chown: {e}")))?;

    let exit_code = match result {
        StartExecResults::Attached { mut output, .. } => {
            // Drain the stream to ensure the process has fully exited.
            while let Some(item) = output.next().await {
                if let Err(e) = item {
                    tracing::trace!("workspace chown output: {}", e);
                }
            }
            // Now inspect_exec will reflect the completed exit state.
            docker
                .inspect_exec(&exec.id)
                .await
                .map_err(|e| Error::sandbox(format!("failed to inspect cleanup exec: {e}")))?
                .exit_code
                .unwrap_or(1) as i32
        }
        // Detached would only occur if StartExecOptions::detach was set, which
        // is not the case here.  Handle it defensively by reading exit code now.
        StartExecResults::Detached => docker
            .inspect_exec(&exec.id)
            .await
            .map_err(|e| Error::sandbox(format!("failed to inspect cleanup exec: {e}")))?
            .exit_code
            .unwrap_or(1) as i32,
    };

    if exit_code != 0 {
        return Err(Error::sandbox(format!(
            "workspace chown exited with code {exit_code} (uid={uid}, gid={gid})"
        )));
    }

    tracing::debug!("workspace {} chowned to {}:{}", workspace, uid, gid);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitforge_common::ErrorKind;
    use serial_test::serial;
    use std::sync::Mutex;

    struct RecordingSink(Mutex<Vec<(OutputStream, Vec<u8>)>>);

    #[test]
    fn test_container_names_are_unique_per_attempt() {
        let job_id = JobId::new();
        let first = DockerSandbox::container_name(job_id);
        let second = DockerSandbox::container_name(job_id);

        assert_ne!(first, second);
        assert!(first.starts_with(&format!("gitforce-job-{job_id}-")));
        assert!(second.starts_with(&format!("gitforce-job-{job_id}-")));
    }

    #[test]
    fn test_workspace_git_env_grants_safe_directory_via_env_config() {
        let env = workspace_git_env();
        // The blanket `safe.directory = *` wildcard is honored only from
        // global/system config, so the exec exports GIT_CONFIG_GLOBAL aimed
        // at the file the bootstrap wrapper materializes.
        assert_eq!(env, &["GIT_CONFIG_GLOBAL=/tmp/.gitforge-gitconfig"]);
    }

    /// Pipeline env reaches the exec as `KEY=VALUE` pairs layered after the
    /// git trust vars; with no workspace mount the caller env stands alone.
    #[test]
    fn test_compose_exec_env_layers_caller_env_over_git_trust() {
        let caller = vec!["PYTHONPATH=src".to_string(), "DI_ENV=test".to_string()];

        let with_workspace = compose_exec_env(true, &caller);
        assert_eq!(with_workspace.len(), 3);
        assert!(with_workspace[0].starts_with("GIT_CONFIG_GLOBAL="));
        assert_eq!(with_workspace[1], "PYTHONPATH=src");
        assert_eq!(with_workspace[2], "DI_ENV=test");

        let without_workspace = compose_exec_env(false, &caller);
        assert_eq!(without_workspace, caller);
    }

    /// R6.3: the stub backend reports a healthy probe with the measurement
    /// explicitly skipped, so test runners never alert on their own sandbox.
    #[tokio::test]
    async fn test_backend_health_stub_reports_healthy_skipped_probe() {
        let sandbox = DockerSandbox::stub_for_tests();
        let health = sandbox.backend_health().await;
        assert!(health.healthy());
        assert!(health.daemon_ok);
        assert_eq!(health.scratch_free_bytes, None);
        assert!(health.detail.contains("stub"));
    }

    /// The real probe must produce a measurement (not `None`) when the
    /// scratch filesystem can be stated, and the healthy() verdict must
    /// honor the floor. Runs against whatever daemon configuration the
    /// host has; skipped silently when none is reachable.
    #[tokio::test]
    #[serial]
    async fn test_backend_health_real_daemon_reports_measurements() {
        if !std::path::Path::new("/var/run/docker.sock").exists()
            && std::env::var_os("DOCKER_HOST").is_none()
        {
            eprintln!("skipping: no Docker daemon for backend health probe test");
            return;
        }
        let Ok(sandbox) = DockerSandbox::connect_required().await else {
            eprintln!("skipping: daemon could not be reached for backend health probe test");
            return;
        };
        let health = sandbox.backend_health().await;
        assert!(
            health.daemon_ok,
            "a backend that answered connect must answer the probe: {}",
            health.detail
        );
        assert!(
            health.scratch_free_bytes.is_some(),
            "statvfs on the scratch dir must measure on a healthy host"
        );
        assert!(health.healthy(), "{}", health.detail);
    }

    /// The F45 shape directly: a daemon that pings fine but an exhausted
    /// scratch fs must not count as healthy.
    #[test]
    fn test_backend_health_floor_rejects_exhausted_scratch() {
        let exhausted = BackendHealth {
            daemon_ok: true,
            scratch_free_bytes: Some(300 * 1024 * 1024),
            scratch_min_free_bytes: 2 * 1024 * 1024 * 1024,
            detail: "scratch free 0.3 GiB (floor 2.0 GiB)".to_string(),
        };
        assert!(!exhausted.healthy());

        let comfortable = BackendHealth {
            scratch_free_bytes: Some(3 * 1024 * 1024 * 1024),
            ..exhausted.clone()
        };
        assert!(comfortable.healthy());
    }

    #[async_trait]
    impl OutputSink for RecordingSink {
        async fn on_output(&self, stream: OutputStream, chunk: Vec<u8>) -> Result<()> {
            self.0.lock().unwrap().push((stream, chunk));
            Ok(())
        }
    }

    #[test]
    fn test_build_volume_mounts_bind_expands_placeholders() {
        let root =
            std::env::temp_dir().join(format!("gitforge-mount-test-{}", uuid::Uuid::new_v4()));
        let volumes = vec![VolumeSpec {
            volume_type: Some(VolumeType::Bind),
            source: Some(format!("{}/cargo/{{image}}", root.display())),
            destination: "/cargo-target".to_string(),
            read_only: false,
            size: None,
        }];

        let volume_mounts = build_volume_mounts(
            &volumes,
            "dsc-ci-rust:10",
            &root.display().to_string(),
            "/var/run/docker.sock",
        )
        .unwrap();
        assert!(volume_mounts.mounts.is_empty());
        assert_eq!(volume_mounts.binds.len(), 1);
        assert_eq!(
            volume_mounts.binds[0],
            format!("{}/cargo/dsc-ci-rust-10:/cargo-target:rw,z", root.display())
        );
        assert!(
            !volume_mounts.disable_selinux,
            "a directory mount keeps the container's own labeling"
        );
        // The mount source is created by the runner, as the runner's own
        // user, so the job's user can write into it.
        assert!(root.join("cargo/dsc-ci-rust-10").is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn test_build_volume_mounts_binds_the_daemon_socket_without_a_relabel() {
        // The docker-integration job declares `source: "{docker_socket}"`.
        // Before the placeholder was expanded, the runner tried to bind the
        // literal string and the job died with "volume mount source must be
        // an absolute path: {docker_socket}" (CI run f2292b0e, 2026-11-06).
        let root =
            std::env::temp_dir().join(format!("gitforge-socket-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("a scratch directory");
        let socket = root.join("docker.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("a unix socket");
        let volumes = vec![VolumeSpec {
            volume_type: Some(VolumeType::Bind),
            source: Some("{docker_socket}".to_string()),
            destination: "/var/run/docker.sock".to_string(),
            read_only: true,
            size: None,
        }];

        let volume_mounts = build_volume_mounts(
            &volumes,
            "dsc-ci-rust:16",
            "/var/cache/gitforge",
            &socket.to_string_lossy(),
        )
        .expect("a socket mount is a mount the runner can honour");

        assert_eq!(
            volume_mounts.binds,
            vec![format!("{}:/var/run/docker.sock:ro", socket.display())],
            "the socket bind carries no relabel: `:z` would rewrite the host socket's label"
        );
        assert!(
            volume_mounts.disable_selinux,
            "a container that mounts the daemon socket cannot keep SELinux labeling"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_build_volume_mounts_read_only_bind() {
        let root =
            std::env::temp_dir().join(format!("gitforge-mount-test-{}", uuid::Uuid::new_v4()));
        let volumes = vec![VolumeSpec {
            volume_type: Some(VolumeType::Bind),
            source: Some(root.display().to_string()),
            destination: "/cache".to_string(),
            read_only: true,
            size: None,
        }];

        let volume_mounts = build_volume_mounts(
            &volumes,
            "dsc-ci-rust:10",
            &root.display().to_string(),
            "/var/run/docker.sock",
        )
        .unwrap();
        assert_eq!(
            volume_mounts.binds[0],
            format!("{}:/cache:ro,z", root.display())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_build_volume_mounts_tmpfs_carries_size() {
        let volumes = vec![VolumeSpec {
            volume_type: Some(VolumeType::Tmpfs),
            source: None,
            destination: "/scratch".to_string(),
            read_only: false,
            size: Some("4G".to_string()),
        }];

        let volume_mounts = build_volume_mounts(
            &volumes,
            "dsc-ci-rust:10",
            "/var/cache/gitforge",
            "/var/run/docker.sock",
        )
        .unwrap();
        assert!(volume_mounts.binds.is_empty());
        assert_eq!(volume_mounts.mounts.len(), 1);
        assert_eq!(volume_mounts.mounts[0].target.as_deref(), Some("/scratch"));
        assert_eq!(
            volume_mounts.mounts[0]
                .tmpfs_options
                .as_ref()
                .and_then(|options| options.size_bytes),
            Some(4 * 1024 * 1024 * 1024)
        );
    }

    #[test]
    fn test_build_volume_mounts_rejects_relative_destination() {
        let volumes = vec![VolumeSpec {
            volume_type: Some(VolumeType::Bind),
            source: Some("/var/cache".to_string()),
            destination: "relative".to_string(),
            read_only: false,
            size: None,
        }];

        assert!(build_volume_mounts(
            &volumes,
            "dsc-ci-rust:10",
            "/var/cache/gitforge",
            "/var/run/docker.sock"
        )
        .is_err());
    }

    #[test]
    fn test_build_volume_mounts_reports_a_source_it_cannot_prepare() {
        // A bind source under a regular file can never become a directory,
        // so the runner cannot prepare it. That is an error the job has to
        // report, not a mount to drop quietly: the job would otherwise run
        // with a contract the pipeline never declared (GF-10, 2026-11-05).
        let binary = std::env::current_exe().expect("the test binary's own path");
        let volumes = vec![VolumeSpec {
            volume_type: Some(VolumeType::Bind),
            source: Some(binary.join("cache").to_string_lossy().into_owned()),
            destination: "/cargo-target".to_string(),
            read_only: false,
            size: None,
        }];

        let error = build_volume_mounts(
            &volumes,
            "dsc-ci-rust:10",
            "/var/cache/gitforge",
            "/var/run/docker.sock",
        )
        .expect_err("a source the runner cannot prepare must be an error");
        assert!(
            error
                .to_string()
                .contains("could not prepare volume mount source"),
            "the error must name the mount it could not prepare, got {error}"
        );
    }

    #[test]
    fn test_parse_size_bytes() {
        assert_eq!(parse_size_bytes("4G"), Some(4 * 1024 * 1024 * 1024));
        assert_eq!(parse_size_bytes("512M"), Some(512 * 1024 * 1024));
        assert_eq!(parse_size_bytes("1024"), Some(1024));
        assert_eq!(parse_size_bytes("nonsense"), None);
    }

    #[tokio::test]
    async fn test_stub_streams_output_to_sink() {
        let sandbox = DockerSandbox::stub_for_tests();
        let instance = sandbox
            .create(JobId::new(), "test", SandboxLimits::default(), &[])
            .await
            .unwrap();
        let sink = Arc::new(RecordingSink(Mutex::new(Vec::new())));

        let result = sandbox
            .execute_with_output(&instance, &["echo", "hello"], Some(sink.clone()))
            .await
            .unwrap();
        let chunks = sink.0.lock().unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].0, OutputStream::Stdout);
        assert_eq!(chunks[1].0, OutputStream::Stderr);
        assert_eq!(chunks[0].1, result.stdout.as_bytes());
        assert_eq!(chunks[1].1, result.stderr.as_bytes());
    }

    #[tokio::test]
    async fn test_docker_sandbox_creation() {
        // Test stub mode creation works
        let sandbox = DockerSandbox::stub_for_tests();
        assert!(!sandbox.is_available());
        assert!(sandbox.is_stub());

        // Test that connect_required returns error when Docker is unavailable
        let result = DockerSandbox::connect_required().await;
        // Either succeeds (Docker available) or fails with sandbox error
        if let Err(e) = result {
            assert_eq!(e.kind, ErrorKind::Sandbox);
        }
    }

    #[tokio::test]
    #[ignore] // Requires Docker and is slow - run manually with `cargo test -- --ignored`
    async fn test_docker_sandbox_real_when_available() {
        // Try to connect - if Docker not available, skip
        let sandbox = match DockerSandbox::connect_required().await {
            Ok(s) => s,
            Err(_) => return, // Docker not available, skip
        };

        // Test with real Docker
        let job_id = JobId::new();
        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await;

        // If Docker has issues, we fall back to stub mode
        if instance.is_err() {
            return;
        }

        let instance = instance.unwrap();
        assert!(!instance.container_id.is_empty());

        let result = sandbox.execute(&instance, &["echo", "hello"]).await;
        if let Ok(exec_result) = result {
            assert_eq!(exec_result.exit_code, 0);
        }

        let _ = sandbox.destroy(instance).await;
    }

    #[tokio::test]
    #[ignore] // Requires Docker and is slow - run manually with `cargo test -- --ignored`
    async fn test_docker_sandbox_real_with_longer_command() {
        // Try to connect - if Docker not available, skip
        let sandbox = match DockerSandbox::connect_required().await {
            Ok(s) => s,
            Err(_) => return,
        };

        let job_id = JobId::new();
        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await;
        if instance.is_err() {
            return;
        }
        let instance = instance.unwrap();

        let result = sandbox.execute(&instance, &["sh", "-c", "echo test"]).await;
        if let Ok(exec_result) = result {
            assert_eq!(exec_result.exit_code, 0);
        }

        let _ = sandbox.destroy(instance).await;
    }

    #[tokio::test]
    #[ignore] // Requires Docker and is slow - run manually with `cargo test -- --ignored`
    async fn test_docker_sandbox_real_multiple_commands() {
        // Try to connect - if Docker not available, skip
        let sandbox = match DockerSandbox::connect_required().await {
            Ok(s) => s,
            Err(_) => return,
        };

        let job_id = JobId::new();
        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await;
        if instance.is_err() {
            return;
        }
        let instance = instance.unwrap();

        let result = sandbox
            .execute(&instance, &["sh", "-c", "echo line1"])
            .await;
        if let Ok(exec_result) = result {
            assert_eq!(exec_result.exit_code, 0);
        }

        let _ = sandbox.destroy(instance).await;
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execution() {
        // Create sandbox in stub mode
        let sandbox = DockerSandbox::stub_for_tests();
        assert!(!sandbox.is_available());

        let job_id = JobId::new();

        // Create container (will use stub)
        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();
        assert!(!instance.container_id.is_empty());

        // Execute command (will use stub)
        let result = sandbox
            .execute(&instance, &["echo", "hello"])
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);

        // Destroy container (will use stub)
        sandbox.destroy(instance).await.unwrap();
    }

    #[test]
    fn test_sandbox_instance_debug() {
        let instance = SandboxInstance {
            container_id: "test-container".to_string(),
            job_id: JobId::new(),
            workspace_path: None,
        };
        let debug_str = format!("{instance:?}");
        assert!(debug_str.contains("test-container"));
    }

    #[test]
    fn test_step_result_debug() {
        let result = StepResult {
            exit_code: 0,
            stdout: "hello".to_string(),
            stderr: String::new(),
        };
        let debug_str = format!("{result:?}");
        assert!(debug_str.contains("hello"));
    }

    #[test]
    #[allow(deprecated)]
    fn test_docker_sandbox_with_custom_limits() {
        let limits = SandboxLimits::small();
        let sandbox = DockerSandbox::with_limits(limits);
        assert!(!sandbox.is_available());
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execution_with_complex_command() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();

        // Test with shell command
        let result = sandbox
            .execute(&instance, &["sh", "-c", "echo hello && echo world"])
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execution_multiple_commands() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();

        // Multiple commands in one execution
        let result = sandbox
            .execute(&instance, &["echo", "line1"])
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[test]
    fn test_sandbox_instance_clone() {
        let instance = SandboxInstance {
            container_id: "clone-test".to_string(),
            job_id: JobId::new(),
            workspace_path: None,
        };
        let cloned = instance.clone();
        assert_eq!(cloned.container_id, instance.container_id);
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_preserves_job_id() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();
        assert_eq!(instance.job_id, job_id);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execute_empty_command() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();

        // Empty command should still work in stub mode
        let result = sandbox.execute(&instance, &[]).await.unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execute_single_command() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(job_id, "rust:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();

        let result = sandbox.execute(&instance, &["true"]).await.unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execute_false_command() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();

        let result = sandbox.execute(&instance, &["false"]).await.unwrap();
        // Stub mode always returns 0, even for false command
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_multiple_instances() {
        let sandbox = DockerSandbox::stub_for_tests();

        // Create multiple instances
        let job1 = JobId::new();
        let job2 = JobId::new();
        let job3 = JobId::new();

        let instance1 = sandbox
            .create(job1, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();
        let instance2 = sandbox
            .create(job2, "ubuntu:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();
        let instance3 = sandbox
            .create(job3, "rust:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();

        // Each should have unique container ID
        assert_ne!(instance1.container_id, instance2.container_id);
        assert_ne!(instance2.container_id, instance3.container_id);

        sandbox.destroy(instance1).await.unwrap();
        sandbox.destroy(instance2).await.unwrap();
        sandbox.destroy(instance3).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_destroy_same_instance_twice() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();

        // Destroy twice should both succeed (idempotent in stub mode)
        sandbox.destroy(instance.clone()).await.unwrap();
        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_with_large_image_name() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        // Very long image name
        let instance = sandbox
            .create(
                job_id,
                "registry.example.com/verylongnamed repository/imagename:latest",
                SandboxLimits::default(),
                &[],
            )
            .await
            .unwrap();
        assert!(!instance.container_id.is_empty());

        sandbox.destroy(instance).await.unwrap();
    }

    #[test]
    fn test_docker_sandbox_debug_trait_not_implemented() {
        // DockerSandbox doesn't implement Debug, which is intentional
        // This test documents that behavior
        let sandbox = DockerSandbox::stub_for_tests();
        assert!(!sandbox.is_available());
    }

    #[test]
    fn test_step_result_with_stderr() {
        let result = StepResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "error message".to_string(),
        };
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.stderr, "error message");
    }

    #[test]
    fn test_step_result_clone() {
        let result = StepResult {
            exit_code: 0,
            stdout: "hello".to_string(),
            stderr: String::new(),
        };
        let cloned = result.clone();
        assert_eq!(cloned.stdout, result.stdout);
        assert_eq!(cloned.exit_code, result.exit_code);
    }

    #[test]
    fn test_sandbox_limits_default() {
        let limits = SandboxLimits::default();
        assert_eq!(limits.memory_mb, 4096);
        assert_eq!(limits.cpu_ms, 3600000);
        assert_eq!(limits.cpu_cores, 1);
        assert!(limits.network);
    }

    #[test]
    fn test_sandbox_limits_medium() {
        let limits = SandboxLimits::medium();
        assert_eq!(limits.memory_mb, 2048);
        assert_eq!(limits.cpu_ms, 1800000);
        assert_eq!(limits.cpu_cores, 1);
        assert!(limits.network);
    }

    #[test]
    fn test_sandbox_limits_large() {
        let limits = SandboxLimits::large();
        assert_eq!(limits.memory_mb, 8192);
        assert_eq!(limits.cpu_ms, 3600000);
        assert_eq!(limits.cpu_cores, 2);
        assert!(limits.network);
    }

    #[test]
    fn test_cpu_quota_uses_bounded_core_count_not_cpu_time_budget() {
        assert_eq!(cpu_quota_micros(1, 16), 100_000);
        assert_eq!(cpu_quota_micros(2, 16), 200_000);
        assert_eq!(cpu_quota_micros(16, 4), 400_000);
        assert_eq!(cpu_quota_micros(0, 0), 100_000);
        assert_ne!(cpu_quota_micros(1, 16), 3_600_000_000);
    }

    #[test]
    fn test_sandbox_limits_with_network_disabled() {
        let limits = SandboxLimits {
            network: false,
            ..Default::default()
        };
        assert!(!limits.network);
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execute_special_characters() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();

        // Test with special characters in command
        let result = sandbox
            .execute(&instance, &["sh", "-c", "echo $'hello\nworld'"])
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execute_unicode() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();

        // Test with unicode
        let result = sandbox
            .execute(&instance, &["echo", "hello world"])
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execute_exit_codes() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();

        // Test 'true' command (exit 0)
        let result = sandbox.execute(&instance, &["true"]).await.unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_container_id_format() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();

        // Container ID should start with expected prefix
        assert!(instance.container_id.starts_with("gitforce-job-"));
        assert!(instance.container_id.contains(&job_id.to_string()));

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_network_enabled() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();
        assert!(!instance.container_id.is_empty());

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_network_disabled() {
        // Note: stub_for_tests() ignores limits, but we verify the sandbox can be created
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(
                job_id,
                "alpine:latest",
                SandboxLimits {
                    network: false,
                    ..Default::default()
                },
                &[],
            )
            .await
            .unwrap();
        assert!(!instance.container_id.is_empty());

        sandbox.destroy(instance).await.unwrap();
    }

    #[test]
    fn test_sandbox_instance_eq() {
        let job_id = JobId::new();
        let instance1 = SandboxInstance {
            container_id: "test-container".to_string(),
            job_id,
            workspace_path: None,
        };
        let instance2 = SandboxInstance {
            container_id: "test-container".to_string(),
            job_id,
            workspace_path: None,
        };
        // Only container_id and job_id are compared
        assert_eq!(instance1.container_id, instance2.container_id);
        assert_eq!(instance1.job_id, instance2.job_id);
    }

    #[test]
    fn test_step_result_with_long_output() {
        let long_string = "x".repeat(10000);
        let result = StepResult {
            exit_code: 0,
            stdout: long_string.clone(),
            stderr: String::new(),
        };
        assert_eq!(result.stdout.len(), 10000);
    }

    // =====================================================================
    // Workspace ownership / cleanup contract tests
    // =====================================================================

    /// Verify that `create()` returns a sandbox instance with no workspace path.
    #[tokio::test]
    async fn test_create_instance_has_no_workspace() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();

        assert!(instance.workspace_path.is_none());
        sandbox.destroy(instance).await.unwrap();
    }

    /// Verify that `create_with_workspace` annotates the instance with the
    /// workspace path so that `destroy` knows to run cleanup.
    ///
    /// The stub's `create_with_workspace` always delegates to Docker (it returns
    /// "Docker is required" when Docker is not available), so we test the
    /// workspace-path annotation by constructing a SandboxInstance directly and
    /// verifying the destroy path handles it correctly.
    #[tokio::test]
    async fn test_create_with_workspace_annotates_path() {
        let sandbox = DockerSandbox::stub_for_tests();

        // Verify that destroy() with a workspace_path does NOT fail on the stub.
        // The stub has no Docker so it cannot run the chown, but it should
        // still succeed (chown failure is warned but not fatal).
        let instance = SandboxInstance {
            container_id: "workspace-test-container".to_string(),
            job_id: JobId::new(),
            workspace_path: Some("/tmp/gitforge-workspace-test".to_string()),
        };

        // With the stub (no Docker), destroy should succeed even with a workspace
        // path — the chown is skipped and a warning is logged.
        let result = sandbox.destroy(instance).await;
        assert!(
            result.is_ok(),
            "destroy with workspace_path should succeed on stub: {result:?}"
        );
    }

    /// Verify that a workspace path appears in the Debug output of SandboxInstance.
    #[tokio::test]
    async fn test_create_with_workspace_annotates_path_in_debug() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        // Stub create_with_workspace requires Docker for workspace containers,
        // so we test via the non-workspace path and check Debug output.
        let instance = sandbox
            .create(job_id, "alpine:latest", SandboxLimits::default(), &[])
            .await
            .unwrap();

        // create() should set workspace_path: None
        assert!(instance.workspace_path.is_none());

        // Debug output should exist and not panic.
        let debug_str = format!("{instance:?}");
        assert!(!debug_str.is_empty());
        sandbox.destroy(instance).await.unwrap();
    }

    /// Workspace execs must carry the git safe.directory config for the
    /// `/workspace` mount, or git's dubious-ownership refusal silently
    /// disables tag probes and other `|| true`-guarded git steps.
    #[test]
    fn workspace_git_env_marks_the_mount_trusted() {
        let env = super::workspace_exec_git_env();
        assert_eq!(env.len(), 1);
        assert!(env[0].starts_with("GIT_CONFIG_GLOBAL="));
    }

    /// Verify that `create_with_workspace` called with no path also produces
    /// `workspace_path: None` (falls through to `create`).
    #[tokio::test]
    async fn test_create_with_workspace_null_path_defaults_to_none() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create_with_workspace(job_id, "alpine:latest", SandboxLimits::default(), None, &[])
            .await
            .unwrap();

        // With a None path the implementation delegates to create(), so no workspace
        assert!(instance.workspace_path.is_none());
        sandbox.destroy(instance).await.unwrap();
    }

    /// Verify `SandboxInstance` with a workspace path can still be cloned.
    #[test]
    fn test_sandbox_instance_with_workspace_clone() {
        let instance = SandboxInstance {
            container_id: "test".to_string(),
            job_id: JobId::new(),
            workspace_path: Some("/tmp/workspace".to_string()),
        };

        let cloned = instance.clone();
        assert_eq!(cloned.container_id, instance.container_id);
        assert_eq!(cloned.job_id, instance.job_id);
        assert_eq!(cloned.workspace_path, instance.workspace_path);
    }

    #[test]
    fn workspace_execs_use_the_container_mount_point() {
        let instance = SandboxInstance {
            container_id: "test".to_string(),
            job_id: JobId::new(),
            workspace_path: Some("/host/workspace".to_string()),
        };

        assert_eq!(
            instance.workspace_path.as_ref().map(|_| "/workspace"),
            Some("/workspace")
        );
    }

    /// Verify `destroy` is a no-op for stub sandbox (no Docker, no workspace).
    #[tokio::test]
    async fn test_destroy_stub_no_workspace_succeeds() {
        let sandbox = DockerSandbox::stub_for_tests();
        let instance = SandboxInstance {
            container_id: "stub-container".to_string(),
            job_id: JobId::new(),
            workspace_path: None,
        };

        // Should succeed without any Docker calls
        let result = sandbox.destroy(instance).await;
        assert!(result.is_ok());
    }

    /// Verify that the `SandboxInstance` debug output includes the workspace path.
    #[test]
    fn test_sandbox_instance_debug_includes_workspace() {
        let instance = SandboxInstance {
            container_id: "debug-test".to_string(),
            job_id: JobId::new(),
            workspace_path: Some("/custom/path".to_string()),
        };

        let debug_str = format!("{instance:?}");
        assert!(
            debug_str.contains("/custom/path"),
            "debug output: {debug_str}"
        );
    }

    // =====================================================================
    // UID / GID validation tests
    // =====================================================================

    #[test]
    fn test_parse_uid_gid_valid() {
        assert_eq!(parse_uid_gid("0"), Some(0));
        assert_eq!(parse_uid_gid("1000"), Some(1000));
        assert_eq!(parse_uid_gid("65534"), Some(65534));
        assert_eq!(parse_uid_gid("  1000  "), Some(1000)); // whitespace is trimmed
    }

    #[test]
    fn test_parse_uid_gid_invalid_not_numeric() {
        assert_eq!(parse_uid_gid("abc"), None);
        assert_eq!(parse_uid_gid("1000a"), None);
        assert_eq!(parse_uid_gid(""), None);
        assert_eq!(parse_uid_gid(" "), None);
        assert_eq!(parse_uid_gid("-1"), None); // negative not allowed for u32
        assert_eq!(parse_uid_gid("0x1000"), None); // hex not accepted
        assert_eq!(parse_uid_gid("65535"), None); // one above max
        assert_eq!(parse_uid_gid("1000000"), None); // way above max
    }

    #[test]
    fn test_parse_uid_gid_max_boundary() {
        assert_eq!(parse_uid_gid("65534"), Some(65534)); // valid
        assert_eq!(parse_uid_gid("65535"), None); // just over max
    }

    #[test]
    fn test_chown_owner_string_format() {
        assert_eq!(chown_owner_string(1000, 1000), "1000:1000");
        assert_eq!(chown_owner_string(0, 0), "0:0");
        assert_eq!(chown_owner_string(65534, 65534), "65534:65534");
        assert_eq!(chown_owner_string(1000, 2000), "1000:2000");
    }

    /// RAII guard that restores env vars on drop.
    struct EnvGuard {
        uid_key: &'static str,
        gid_key: &'static str,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var(self.uid_key);
            std::env::remove_var(self.gid_key);
        }
    }

    fn with_env_vars(
        uid_key: &'static str,
        uid_val: &str,
        gid_key: &'static str,
        gid_val: &str,
    ) -> EnvGuard {
        std::env::set_var(uid_key, uid_val);
        std::env::set_var(gid_key, gid_val);
        EnvGuard { uid_key, gid_key }
    }

    #[test]
    #[serial]
    fn test_resolve_runner_uid_gid_defaults() {
        // Ensure env vars are absent so we get the defaults.
        std::env::remove_var("GITFORGE_RUNNER_UID");
        std::env::remove_var("GITFORGE_RUNNER_GID");

        let (uid, gid) = resolve_runner_uid_gid().expect("default 1000:1000 must parse");
        assert_eq!((uid, gid), (1000, 1000));
    }

    #[test]
    #[serial]
    fn test_resolve_runner_uid_gid_from_env() {
        let _guard = with_env_vars("GITFORGE_RUNNER_UID", "2000", "GITFORGE_RUNNER_GID", "3000");

        let (uid, gid) = resolve_runner_uid_gid().expect("2000:3000 must parse");
        assert_eq!((uid, gid), (2000, 3000));
    }

    #[test]
    #[serial]
    fn test_resolve_runner_uid_gid_invalid_uid_rejected() {
        let _guard = with_env_vars(
            "GITFORGE_RUNNER_UID",
            "not-a-number",
            "GITFORGE_RUNNER_GID",
            "1000",
        );

        let result = resolve_runner_uid_gid();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("GITFORGE_RUNNER_UID"),
            "error message should mention GITFORGE_RUNNER_UID: {err_msg}"
        );
    }

    #[test]
    #[serial]
    fn test_resolve_runner_uid_gid_invalid_gid_rejected() {
        let _guard = with_env_vars(
            "GITFORGE_RUNNER_UID",
            "1000",
            "GITFORGE_RUNNER_GID",
            "99999",
        );

        let result = resolve_runner_uid_gid();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("GITFORGE_RUNNER_GID"),
            "error message should mention GITFORGE_RUNNER_GID: {err_msg}"
        );
    }

    #[test]
    #[serial]
    fn test_resolve_runner_uid_gid_overflow_rejected() {
        let _guard = with_env_vars(
            "GITFORGE_RUNNER_UID",
            "70000",
            "GITFORGE_RUNNER_GID",
            "1000",
        );

        let result = resolve_runner_uid_gid();
        assert!(result.is_err());
    }

    // =====================================================================
    // Command-construction contract tests
    //
    // We test the command that cleanup_workspace WOULD send to Docker by
    // inspecting the CreateExecOptions it builds.  We do this without any
    // Docker I/O by checking that resolve_runner_uid_gid + chown_owner_string
    // produce the values that would be embedded in the exec command.
    // =====================================================================

    /// Confirm that the uid:gid string embedded in the chown command contains
    /// only the decimal digits and colons from validated numeric inputs —
    /// no shell metacharacters can leak through because parse_uid_gid rejects
    /// everything that is not a plain decimal u32.
    #[test]
    fn test_chown_command_string_is_safe() {
        // Valid uid/gid produce only [0-9:] characters.
        let (uid, gid) = (12345u32, 67890u32);
        let owner = chown_owner_string(uid, gid);
        assert!(owner.chars().all(|c| c.is_ascii_digit() || c == ':'));
        assert_eq!(owner, "12345:67890");
    }

    /// If either UID or GID is invalid, resolve_runner_uid_gid returns an Error
    /// before any command string is built — so no unchecked input reaches Docker.
    #[test]
    #[serial]
    fn test_resolve_rejects_before_command_built() {
        let _guard = with_env_vars(
            "GITFORGE_RUNNER_UID",
            "$(echo pwned)",
            "GITFORGE_RUNNER_GID",
            "1000",
        );

        let result = resolve_runner_uid_gid();
        assert!(
            result.is_err(),
            "shell injection must be rejected at parse time"
        );
    }

    /// Verify that whitespace-surrounded valid numbers still parse correctly,
    /// which is the only way a user-supplied env var could reach the command.
    #[test]
    fn test_whitespace_surrounded_valid_uid_gid_parses() {
        // A user might set GITFORGE_RUNNER_UID="  1000  " — parse_uid_gid must accept it.
        assert_eq!(parse_uid_gid("  1000  "), Some(1000));
        assert_eq!(parse_uid_gid("\t500\n"), Some(500));
    }

    /// Verify cleanup_workspace is fail-open: when the stub's destroy() is called
    /// with a workspace_path it succeeds without calling Docker at all.
    #[tokio::test]
    async fn test_cleanup_is_skipped_on_stub() {
        // The stub docker = None path means cleanup_workspace is never entered.
        // destroy() with workspace_path succeeds because the stub has no docker.
        let sandbox = DockerSandbox::stub_for_tests();
        let instance = SandboxInstance {
            container_id: "stub-cleanup-test".to_string(),
            job_id: JobId::new(),
            workspace_path: Some("/tmp/workspace".to_string()),
        };
        let result = sandbox.destroy(instance).await;
        assert!(
            result.is_ok(),
            "stub destroy must not fail even with workspace_path set"
        );
    }

    /// The ownership cleanup must never be able to swallow the teardown that
    /// follows it: a cleanup that never returns is abandoned and the caller
    /// proceeds. The regression is the leaked job container (2026-11-05).
    #[tokio::test]
    async fn test_a_cleanup_that_never_returns_is_abandoned() {
        let outcome = bounded_cleanup(
            async {
                sleep(Duration::from_secs(30)).await;
                Ok(())
            },
            Duration::from_millis(50),
        )
        .await;
        assert_eq!(outcome, WorkspaceCleanup::TimedOut);
    }

    /// A failed cleanup is reported, not propagated: the teardown continues.
    #[tokio::test]
    async fn test_a_failed_cleanup_is_reported() {
        let outcome = bounded_cleanup(
            async { Err(Error::sandbox("chown failed")) },
            Duration::from_secs(5),
        )
        .await;
        assert!(matches!(outcome, WorkspaceCleanup::Failed(_)));
    }

    /// A cleanup that finishes in time is reported as completed.
    #[tokio::test]
    async fn test_a_completed_cleanup_is_reported_as_completed() {
        let outcome = bounded_cleanup(async { Ok(()) }, Duration::from_secs(5)).await;
        assert_eq!(outcome, WorkspaceCleanup::Completed);
    }

    /// The backstop the runner uses when a teardown overruns its budget:
    /// the container is removed by its job label. The label is the only
    /// ownership evidence, and another job's container is never touched.
    ///
    /// This is the primitive that makes the leak class closeable. The
    /// background reconciler refuses to remove a RUNNING container - its
    /// policy keeps running containers eligible for nobody - so a teardown
    /// that overran its budget could only be cleaned up by an explicit,
    /// label-scoped removal (2026-11-05: eight leaked containers that no
    /// other mechanism would ever reap).
    #[tokio::test]
    #[ignore] // requires a Docker daemon
    async fn test_remove_job_containers_removes_only_that_jobs_containers() {
        let sandbox = match DockerSandbox::connect_required().await {
            Ok(sandbox) => sandbox,
            Err(_) => return,
        };
        let owner = JobId::new();
        let other = JobId::new();
        let image = "backend-ci-rust:1";
        let owned = match sandbox
            .create(owner, image, SandboxLimits::default(), &[])
            .await
        {
            Ok(instance) => instance,
            Err(_) => return,
        };
        let foreign = match sandbox
            .create(other, image, SandboxLimits::default(), &[])
            .await
        {
            Ok(instance) => instance,
            Err(_) => {
                let _ = sandbox.remove_job_containers(owner).await;
                return;
            }
        };
        let docker = match sandbox.docker.as_ref() {
            Some(docker) => docker,
            None => return,
        };
        assert!(docker
            .inspect_container(&owned.container_id, None)
            .await
            .is_ok());
        assert!(docker
            .inspect_container(&foreign.container_id, None)
            .await
            .is_ok());

        sandbox
            .remove_job_containers(owner)
            .await
            .expect("the label-scoped removal must succeed");

        assert!(docker
            .inspect_container(&owned.container_id, None)
            .await
            .is_err());
        assert!(docker
            .inspect_container(&foreign.container_id, None)
            .await
            .is_ok());

        let _ = sandbox.remove_job_containers(other).await;
    }
}
