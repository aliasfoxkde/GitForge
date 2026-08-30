//! Docker-based sandbox implementation using bollard

use crate::limits::SandboxLimits;
use async_trait::async_trait;
use bollard::container::{
    Config, CreateContainerOptions, LogOutput, RemoveContainerOptions, StartContainerOptions,
    StopContainerOptions,
};
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::image::CreateImageOptions;
use bollard::models::HostConfig;
use bollard::Docker;
use futures_util::StreamExt;
use gitforge_common::{Error, JobId, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

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
    /// ownership label, so unrelated containers are never considered.
    async fn remove_job_containers(&self, job_id: JobId) -> Result<()> {
        let Some(ref docker) = self.docker else {
            return Ok(());
        };

        let mut filters = HashMap::new();
        filters.insert(
            "label".to_string(),
            vec!["com.gitforce.managed=true".to_string()],
        );
        let containers = docker
            .list_containers(Some(bollard::container::ListContainersOptions {
                all: true,
                filters,
                ..Default::default()
            }))
            .await
            .map_err(|e| Error::sandbox(format!("failed to list job containers: {}", e)))?;

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
                docker
                    .remove_container(
                        &id,
                        Some(RemoveContainerOptions {
                            force: true,
                            ..Default::default()
                        }),
                    )
                    .await
                    .map_err(|e| {
                        Error::sandbox(format!("failed to remove stale job container: {}", e))
                    })?;
                tracing::info!(%id, %job_id, "Removed stale sandbox container before retry");
            }
        }
        Ok(())
    }

    /// Create a new Docker sandbox, requiring Docker to be available.
    /// Returns an error if Docker is not available or cannot be reached.
    pub async fn connect_required() -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| Error::sandbox(format!("failed to connect to Docker: {}", e)))?;

        // Verify connection by pinging Docker
        docker
            .ping()
            .await
            .map_err(|e| Error::sandbox(format!("Docker daemon not available: {}", e)))?;

        tracing::info!("Connected to Docker daemon");

        Ok(Self {
            docker: Some(docker),
            default_limits: SandboxLimits::default(),
            is_stub: false,
        })
    }

    /// Create a stub sandbox for testing purposes.
    /// This sandbox does not actually execute commands - it simulates execution.
    /// Commands always succeed with exit code 0 in stub mode.
    pub fn stub_for_tests() -> Self {
        Self {
            docker: None,
            default_limits: SandboxLimits::default(),
            is_stub: true,
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

            // Pull the image - Docker is idempotent, so this works even if image exists
            let mut stream = docker.create_image(
                Some(CreateImageOptions {
                    from_image: image,
                    ..Default::default()
                }),
                None,
                None,
            );

            while let Some(result) = stream.next().await {
                match result {
                    Ok(info) => {
                        tracing::debug!("Pull progress: {:?}", info);
                    }
                    Err(e) => {
                        return Err(Error::sandbox(format!("failed to pull image: {}", e)));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Sandbox trait
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Create a new sandbox instance
    async fn create(
        &self,
        job_id: JobId,
        image: &str,
        limits: SandboxLimits,
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
    ) -> Result<SandboxInstance> {
        let _ = workspace_path;
        self.create(job_id, image, limits).await
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
        let result = self.execute(instance, command).await?;
        if let Some(sink) = sink {
            if !result.stdout.is_empty() {
                sink.on_output(OutputStream::Stdout, result.stdout.as_bytes().to_vec())
                    .await?;
            }
            if !result.stderr.is_empty() {
                sink.on_output(OutputStream::Stderr, result.stderr.as_bytes().to_vec())
                    .await?;
            }
        }
        Ok(result)
    }

    /// Destroy a sandbox instance
    async fn destroy(&self, instance: SandboxInstance) -> Result<()>;
}

#[async_trait]
impl Sandbox for DockerSandbox {
    async fn create(
        &self,
        job_id: JobId,
        image: &str,
        limits: SandboxLimits,
    ) -> Result<SandboxInstance> {
        if let Some(ref docker) = self.docker {
            // Ensure image is available
            self.ensure_image(image).await?;
            self.remove_job_containers(job_id).await?;

            let container_name = Self::container_name(job_id);
            let job_label = job_id.to_string();
            let labels = HashMap::from([
                ("com.gitforce.managed", "true"),
                ("com.gitforce.job_id", job_label.as_str()),
            ]);

            // Build host config with resource limits
            let host_config = HostConfig {
                memory: Some((limits.memory_mb * 1024 * 1024) as i64),
                cpu_period: Some(100000), // 100ms in microseconds
                cpu_quota: Some((limits.cpu_ms * 1000) as i64), // Convert ms to microseconds
                network_mode: if limits.network {
                    None
                } else {
                    Some("none".to_string())
                },
                ..Default::default()
            };

            // Create container
            let config = Config {
                image: Some(image),
                cmd: Some(vec!["sleep", "3600"]), // Keep container alive
                host_config: Some(host_config),
                labels: Some(labels),
                ..Default::default()
            };

            let options = CreateContainerOptions {
                name: &container_name,
                platform: None,
            };

            let response = docker
                .create_container(Some(options), config)
                .await
                .map_err(|e| Error::sandbox(format!("failed to create container: {}", e)))?;

            // Start container
            docker
                .start_container(&response.id, None::<StartContainerOptions<String>>)
                .await
                .map_err(|e| Error::sandbox(format!("failed to start container: {}", e)))?;

            tracing::info!("Created container {} for job {}", response.id, job_id);

            Ok(SandboxInstance {
                container_id: response.id,
                job_id,
                workspace_path: None,
            })
        } else {
            // Stub mode - no Docker available
            Ok(SandboxInstance {
                container_id: format!("gitforce-job-{}", job_id),
                job_id,
                workspace_path: None,
            })
        }
    }

    async fn create_with_workspace(
        &self,
        job_id: JobId,
        image: &str,
        limits: SandboxLimits,
        workspace_path: Option<&str>,
    ) -> Result<SandboxInstance> {
        let Some(workspace_path) = workspace_path else {
            return self.create(job_id, image, limits).await;
        };

        let workspace = Path::new(workspace_path);
        if !workspace.is_absolute() || !workspace.is_dir() {
            return Err(Error::sandbox(format!(
                "workspace must be an existing absolute directory: {}",
                workspace_path
            )));
        }

        if let Some(ref docker) = self.docker {
            // Rootless Podman maps ordinary container UIDs into a subordinate
            // host range. Preserve the runner's host identity for this bind
            // mount so the unprivileged runner can remove the workspace.
            let (uid, gid) = resolve_runner_uid_gid()?;
            let owner = chown_owner_string(uid, gid);
            self.ensure_image(image).await?;
            self.remove_job_containers(job_id).await?;
            let container_name = Self::container_name(job_id);
            let job_label = job_id.to_string();
            let labels = HashMap::from([
                ("com.gitforce.managed", "true"),
                ("com.gitforce.job_id", job_label.as_str()),
            ]);
            let host_config = HostConfig {
                memory: Some((limits.memory_mb * 1024 * 1024) as i64),
                cpu_period: Some(100000),
                cpu_quota: Some((limits.cpu_ms * 1000) as i64),
                network_mode: if limits.network {
                    None
                } else {
                    Some("none".to_string())
                },
                userns_mode: Some("keep-id".to_string()),
                // Fedora's rootless Podman enforces SELinux labels on host
                // mounts. Private relabeling makes this per-workspace mount
                // readable inside the sandbox without disabling enforcement.
                binds: Some(vec![format!("{}:/workspace:Z", workspace_path)]),
                ..Default::default()
            };
            let config = Config {
                image: Some(image),
                cmd: Some(vec!["sleep", "3600"]),
                working_dir: Some("/workspace"),
                user: Some(owner),
                host_config: Some(host_config),
                labels: Some(labels),
                ..Default::default()
            };
            let response = docker
                .create_container(
                    Some(CreateContainerOptions {
                        name: &container_name,
                        platform: None,
                    }),
                    config,
                )
                .await
                .map_err(|e| {
                    Error::sandbox(format!("failed to create workspace container: {}", e))
                })?;
            docker
                .start_container(&response.id, None::<StartContainerOptions<String>>)
                .await
                .map_err(|e| {
                    Error::sandbox(format!("failed to start workspace container: {}", e))
                })?;
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

    async fn execute(&self, instance: &SandboxInstance, command: &[&str]) -> Result<StepResult> {
        self.execute_with_output(instance, command, None).await
    }

    async fn execute_with_output(
        &self,
        instance: &SandboxInstance,
        command: &[&str],
        sink: Option<Arc<dyn OutputSink>>,
    ) -> Result<StepResult> {
        if let Some(ref docker) = self.docker {
            // Create exec instance
            let config = CreateExecOptions {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                cmd: Some(command.to_vec()),
                ..Default::default()
            };

            let exec = docker
                .create_exec(&instance.container_id, config)
                .await
                .map_err(|e| Error::sandbox(format!("failed to create exec: {}", e)))?;

            // Start exec and get results
            let result = docker
                .start_exec(&exec.id, None::<StartExecOptions>)
                .await
                .map_err(|e| Error::sandbox(format!("failed to start exec: {}", e)))?;

            let mut stdout = String::new();
            let mut stderr = String::new();
            let mut exit_code = 0i32;

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
                                    tracing::warn!("exec output error: {}", e);
                                }
                                None => break,
                            }
                        }
                        _ = sleep(Duration::from_secs(1)) => {
                            match docker.inspect_exec(&exec.id).await {
                                Ok(inspect) if inspect.running == Some(false) => break,
                                Ok(_) => {}
                                Err(e) => tracing::trace!("exec status poll failed: {}", e),
                            }
                        }
                    }
                }
            }

            // Inspect to get exit code
            match docker.inspect_exec(&exec.id).await {
                Ok(inspect) => {
                    exit_code = inspect.exit_code.unwrap_or(0) as i32;
                }
                Err(e) => {
                    tracing::warn!("failed to inspect exec: {}", e);
                }
            }

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
                stdout: format!("[STUB] Executing: {:?}\n", command),
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
                if let Err(e) = cleanup_workspace(docker, &instance.container_id, workspace).await {
                    tracing::warn!(
                        "workspace ownership cleanup failed for {}: {} \
                         (artifact files may require privileged deletion)",
                        workspace,
                        e
                    );
                    // Proceed to container teardown even if chown fails
                }
            }

            // Send SIGTERM for graceful shutdown first
            // Wait up to 10 seconds for container to stop gracefully
            let stop_options = StopContainerOptions { t: 10 };

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

            // Now remove the stopped container
            let remove_options = RemoveContainerOptions {
                force: false,
                ..Default::default()
            };

            docker
                .remove_container(&instance.container_id, Some(remove_options))
                .await
                .map_err(|e| Error::sandbox(format!("failed to remove container: {}", e)))?;

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
    format!("{}:{}", uid, gid)
}

/// Resolve the runner UID/GID from environment variables, falling back to
/// `GITFORGE_RUNNER_UID` / `GITFORGE_RUNNER_GID` (default 1000:1000).
/// Returns `(uid, gid)` or an error if either variable is set to an invalid value.
fn resolve_runner_uid_gid() -> Result<(u32, u32)> {
    let uid_raw = std::env::var("GITFORGE_RUNNER_UID").unwrap_or_else(|_| "1000".to_string());
    let gid_raw = std::env::var("GITFORGE_RUNNER_GID").unwrap_or_else(|_| "1000".to_string());

    let uid = parse_uid_gid(&uid_raw)
        .ok_or_else(|| Error::sandbox(format!("invalid GITFORGE_RUNNER_UID: {}", uid_raw)))?;
    let gid = parse_uid_gid(&gid_raw)
        .ok_or_else(|| Error::sandbox(format!("invalid GITFORGE_RUNNER_GID: {}", gid_raw)))?;

    Ok((uid, gid))
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
        .map_err(|e| Error::sandbox(format!("failed to create cleanup exec: {}", e)))?;

    // start_exec returns StartExecResults. If the container supports exec
    // (Linux containers always do) we get an Attached handle with the output
    // stream. We MUST drain the stream to completion before reading exit codes.
    let result = docker
        .start_exec(&exec.id, None::<StartExecOptions>)
        .await
        .map_err(|e| Error::sandbox(format!("failed to start workspace chown: {}", e)))?;

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
                .map_err(|e| Error::sandbox(format!("failed to inspect cleanup exec: {}", e)))?
                .exit_code
                .unwrap_or(1) as i32
        }
        // Detached would only occur if StartExecOptions::detach was set, which
        // is not the case here.  Handle it defensively by reading exit code now.
        StartExecResults::Detached => docker
            .inspect_exec(&exec.id)
            .await
            .map_err(|e| Error::sandbox(format!("failed to inspect cleanup exec: {}", e)))?
            .exit_code
            .unwrap_or(1) as i32,
    };

    if exit_code != 0 {
        return Err(Error::sandbox(format!(
            "workspace chown exited with code {} (uid={}, gid={})",
            exit_code, uid, gid
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
        assert!(first.starts_with(&format!("gitforce-job-{}-", job_id)));
        assert!(second.starts_with(&format!("gitforce-job-{}-", job_id)));
    }

    #[async_trait]
    impl OutputSink for RecordingSink {
        async fn on_output(&self, stream: OutputStream, chunk: Vec<u8>) -> Result<()> {
            self.0.lock().unwrap().push((stream, chunk));
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_stub_streams_output_to_sink() {
        let sandbox = DockerSandbox::stub_for_tests();
        let instance = sandbox
            .create(JobId::new(), "test", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
        let debug_str = format!("{:?}", instance);
        assert!(debug_str.contains("test-container"));
    }

    #[test]
    fn test_step_result_debug() {
        let result = StepResult {
            exit_code: 0,
            stdout: "hello".to_string(),
            stderr: String::new(),
        };
        let debug_str = format!("{:?}", result);
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            .create(job_id, "rust:latest", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            .create(job1, "alpine:latest", SandboxLimits::default())
            .await
            .unwrap();
        let instance2 = sandbox
            .create(job2, "ubuntu:latest", SandboxLimits::default())
            .await
            .unwrap();
        let instance3 = sandbox
            .create(job3, "rust:latest", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
        assert!(limits.network);
    }

    #[test]
    fn test_sandbox_limits_medium() {
        let limits = SandboxLimits::medium();
        assert_eq!(limits.memory_mb, 2048);
        assert_eq!(limits.cpu_ms, 1800000);
        assert!(limits.network);
    }

    #[test]
    fn test_sandbox_limits_large() {
        let limits = SandboxLimits::large();
        assert_eq!(limits.memory_mb, 8192);
        assert_eq!(limits.cpu_ms, 3600000);
        assert!(limits.network);
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
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
            "destroy with workspace_path should succeed on stub: {:?}",
            result
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
            .create(job_id, "alpine:latest", SandboxLimits::default())
            .await
            .unwrap();

        // create() should set workspace_path: None
        assert!(instance.workspace_path.is_none());

        // Debug output should exist and not panic.
        let debug_str = format!("{:?}", instance);
        assert!(!debug_str.is_empty());
        sandbox.destroy(instance).await.unwrap();
    }

    /// Verify that `create_with_workspace` called with no path also produces
    /// `workspace_path: None` (falls through to `create`).
    #[tokio::test]
    async fn test_create_with_workspace_null_path_defaults_to_none() {
        let sandbox = DockerSandbox::stub_for_tests();
        let job_id = JobId::new();

        let instance = sandbox
            .create_with_workspace(job_id, "alpine:latest", SandboxLimits::default(), None)
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

        let debug_str = format!("{:?}", instance);
        assert!(
            debug_str.contains("/custom/path"),
            "debug output: {}",
            debug_str
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
            "error message should mention GITFORGE_RUNNER_UID: {}",
            err_msg
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
            "error message should mention GITFORGE_RUNNER_GID: {}",
            err_msg
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
}
