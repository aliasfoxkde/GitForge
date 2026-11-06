//! Volume and cache-mount specifications.
//!
//! A pipeline job declares the host mounts it needs (`volumes:` in the job
//! definition); the CI service copies them into the scheduler plan, the
//! scheduler ships them to the runner, and the sandbox turns them into
//! container mounts. One shared type keeps that chain honest instead of
//! letting each hop re-derive the shape.

use serde::{Deserialize, Serialize};

/// Kind of host mount requested by a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VolumeType {
    /// Host path bind-mounted into the container.
    #[default]
    Bind,
    /// In-memory filesystem mounted at the destination.
    Tmpfs,
    /// Container-managed named volume.
    Volume,
}

/// One mount requested by a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeSpec {
    /// Mount kind. When omitted it is inferred: a mount with a `source` is
    /// a bind, a mount without one is a tmpfs.
    #[serde(rename = "type", default)]
    pub volume_type: Option<VolumeType>,
    /// Host path (bind) or volume name (named volume). The path may carry
    /// the `{image}` placeholder, which the sandbox expands to the job's
    /// image reference so a new image tag never reuses another tag's
    /// build artifacts.
    #[serde(default)]
    pub source: Option<String>,
    /// Absolute path inside the container.
    pub destination: String,
    /// Mount read-only.
    #[serde(default)]
    pub read_only: bool,
    /// Size limit for a tmpfs mount (`4G`, `512M`, ...).
    #[serde(default)]
    pub size: Option<String>,
}

impl VolumeSpec {
    /// The mount kind actually used, inferring it when `type` is absent.
    pub fn resolved_type(&self) -> VolumeType {
        match self.volume_type {
            Some(kind) => kind,
            None if self.source.is_some() => VolumeType::Bind,
            None => VolumeType::Tmpfs,
        }
    }

    /// The host source with the `{image}`, `{cache_root}` and
    /// `{docker_socket}` placeholders expanded against the job's image, the
    /// runner's cache root, and the runner's daemon socket.
    pub fn expanded_source(
        &self,
        image: &str,
        cache_root: &str,
        docker_socket: &str,
    ) -> Option<String> {
        self.source.as_ref().map(|source| {
            source
                .replace("{image}", &image_cache_key(image))
                .replace("{cache_root}", cache_root)
                .replace("{docker_socket}", docker_socket)
        })
    }

    /// Reject a mount the sandbox cannot honour.
    pub fn validate(&self) -> Result<(), String> {
        if !self.destination.starts_with('/') {
            return Err(format!(
                "volume destination must be an absolute path: {}",
                self.destination
            ));
        }
        match self.resolved_type() {
            VolumeType::Bind | VolumeType::Volume => {
                if self
                    .source
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .is_empty()
                {
                    return Err(format!(
                        "volume mount requires a source: {}",
                        self.destination
                    ));
                }
            }
            VolumeType::Tmpfs => {}
        }
        Ok(())
    }
}

/// A compiled-artifact cache mounted into a job's container.
///
/// The runner bind-mounts a host directory at `mount_path` inside the
/// container and exports `env_var` pointing at it, so consecutive runs on
/// the same image reuse compiled artifacts instead of rebuilding the
/// world. The host directory is keyed by the image reference, so a rebuilt
/// image (new tag) starts from an empty cache rather than inheriting
/// artifacts compiled by a different toolchain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobCache {
    /// Host directory holding the cache. Supports the `{cache_root}` and
    /// `{image}` placeholders, which the runner expands against its own
    /// cache root and the job's image reference.
    #[serde(default = "default_cache_source")]
    pub source: String,
    /// Path inside the container where the cache is mounted.
    #[serde(default = "default_cache_mount_path")]
    pub mount_path: String,
    /// Environment variable exported into the job pointing at the mount.
    #[serde(default = "default_cache_env_var")]
    pub env_var: String,
    /// Mount the cache read-only.
    #[serde(default)]
    pub read_only: bool,
}

fn default_cache_source() -> String {
    "{cache_root}/cargo/{image}".to_string()
}

fn default_cache_mount_path() -> String {
    "/cargo-target".to_string()
}

fn default_cache_env_var() -> String {
    "CARGO_TARGET_DIR".to_string()
}

impl Default for JobCache {
    fn default() -> Self {
        Self {
            source: default_cache_source(),
            mount_path: default_cache_mount_path(),
            env_var: default_cache_env_var(),
            read_only: false,
        }
    }
}

impl JobCache {
    /// The bind mount this cache expands to.
    pub fn to_volume(&self) -> VolumeSpec {
        VolumeSpec {
            volume_type: Some(VolumeType::Bind),
            source: Some(self.source.clone()),
            destination: self.mount_path.clone(),
            read_only: self.read_only,
            size: None,
        }
    }
}

/// Where the runner keeps job caches when neither an override nor a home
/// directory is available. Overridable with `GITFORGE_CACHE_ROOT`; the
/// fallback matches the usual system cache path.
pub const DEFAULT_CACHE_ROOT: &str = "/var/cache/gitforge";

/// The cache root for the given environment.
///
/// The override wins. Failing that, the home directory's conventional
/// cache location. Only when neither is available does the system path
/// apply.
///
/// The home directory is part of the contract, not a convenience:
/// `docs/RUNBOOK.md` documents the default as `~/.cache/gitforge`, and a
/// runner that resolved a path it cannot write lost every job cache
/// without saying so (2026-11-05, ledger row GF-09: the runner resolved
/// `/var/cache/gitforge`, which its own unit cannot write, so the
/// platform's own pipeline cold-built on every run).
pub fn cache_root_with(override_root: Option<&str>, home: Option<&str>) -> String {
    let override_root = override_root.map(str::trim).unwrap_or_default();
    if !override_root.is_empty() {
        return override_root.to_string();
    }
    let home = home.map(str::trim).unwrap_or_default();
    if !home.is_empty() {
        return std::path::Path::new(home)
            .join(".cache")
            .join("gitforge")
            .to_string_lossy()
            .into_owned();
    }
    DEFAULT_CACHE_ROOT.to_string()
}

/// The runner's cache root, taken from the environment.
///
/// Call this once at the edge - where a job starts, and where a container
/// is created - and pass the value down. A decision function that reads
/// process-global state internally inherits whatever any other thread is
/// doing to it at that instant.
///
/// One definition for the whole platform: the runner (which locks the
/// directory a job mounts) and the sandbox (which mounts it) must agree
/// on which host directory a cache points at, or the lock would guard a
/// path nobody writes to.
pub fn cache_root() -> String {
    cache_root_with(
        std::env::var("GITFORGE_CACHE_ROOT").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// The well-known system-wide Docker daemon socket.
pub const DEFAULT_DOCKER_SOCKET: &str = "/var/run/docker.sock";

/// The Docker daemon socket named by a `DOCKER_HOST` value.
///
/// `DOCKER_HOST` wins when it names a unix socket: a rootless podman's
/// socket is not the system-wide path. The scheme is stripped, because the
/// value is used as a host bind source, not as a URL.
pub fn docker_socket_with(docker_host: Option<&str>) -> String {
    docker_host
        .and_then(|host| host.strip_prefix("unix://"))
        .filter(|path| !path.is_empty())
        .unwrap_or(DEFAULT_DOCKER_SOCKET)
        .to_string()
}

/// The Docker daemon socket this runner talks to.
///
/// One definition for the whole platform: the sandbox connects to this
/// socket, the reconciler reconciles this daemon's containers, and a
/// `{docker_socket}` mount binds this socket into the job's container. A
/// runner that bound one daemon's socket while talking to another would run
/// the job against a daemon it does not supervise.
///
/// Call this once at the edge - where a job starts, and where a container
/// is created - and pass the value down.
pub fn docker_socket() -> String {
    docker_socket_with(std::env::var("DOCKER_HOST").ok().as_deref())
}

/// Whether a bind source names a unix socket rather than a directory.
///
/// A socket is not a directory: the runner must not create one in its
/// place, must not relabel it (`:z` rewrites the host socket's SELinux
/// label - a host-state change the runner does not own), and must not lock
/// it like a shared build directory.
#[cfg(unix)]
pub fn is_socket_source(source: &str) -> bool {
    use std::os::unix::fs::FileTypeExt;

    std::fs::metadata(source).is_ok_and(|metadata| metadata.file_type().is_socket())
}

/// Whether a bind source names a unix socket rather than a directory.
#[cfg(not(unix))]
pub fn is_socket_source(_source: &str) -> bool {
    false
}

/// A filesystem-safe key for an image reference.
///
/// Caches are keyed by this value so a rebuilt image (new tag) starts from
/// an empty cache instead of inheriting artifacts compiled by a different
/// toolchain.
pub fn image_cache_key(image: &str) -> String {
    let mut key = String::with_capacity(image.len());
    for character in image.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
            key.push(character);
        } else {
            key.push('-');
        }
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_bind_without_explicit_type() {
        let spec = VolumeSpec {
            volume_type: None,
            source: Some("/var/cache/gitforge-cargo".to_string()),
            destination: "/cargo-target".to_string(),
            read_only: false,
            size: None,
        };
        assert_eq!(spec.resolved_type(), VolumeType::Bind);
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn infers_tmpfs_without_source() {
        let spec = VolumeSpec {
            volume_type: None,
            source: None,
            destination: "/scratch".to_string(),
            read_only: false,
            size: Some("4G".to_string()),
        };
        assert_eq!(spec.resolved_type(), VolumeType::Tmpfs);
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn rejects_relative_destination() {
        let spec = VolumeSpec {
            volume_type: Some(VolumeType::Bind),
            source: Some("/var/cache".to_string()),
            destination: "cargo-target".to_string(),
            read_only: false,
            size: None,
        };
        assert!(spec.validate().is_err());
    }

    #[test]
    fn rejects_bind_without_source() {
        let spec = VolumeSpec {
            volume_type: Some(VolumeType::Bind),
            source: None,
            destination: "/cargo-target".to_string(),
            read_only: false,
            size: None,
        };
        assert!(spec.validate().is_err());
    }

    #[test]
    fn expands_image_placeholder() {
        let spec = VolumeSpec {
            volume_type: Some(VolumeType::Bind),
            source: Some("/var/cache/gitforge-cargo/{image}".to_string()),
            destination: "/cargo-target".to_string(),
            read_only: false,
            size: None,
        };
        assert_eq!(
            spec.expanded_source(
                "dsc-ci-rust:10",
                "/var/cache/gitforge",
                "/var/run/docker.sock"
            )
            .as_deref(),
            Some("/var/cache/gitforge-cargo/dsc-ci-rust-10")
        );
    }

    #[test]
    fn image_cache_key_is_filesystem_safe() {
        assert_eq!(image_cache_key("dsc-ci-rust:10"), "dsc-ci-rust-10");
        assert_eq!(
            image_cache_key("registry/repo/name:v1"),
            "registry-repo-name-v1"
        );
        assert_eq!(image_cache_key("a_b.c"), "a_b.c");
    }

    #[test]
    fn the_cache_override_wins_over_the_home_directory() {
        assert_eq!(
            cache_root_with(Some("/nas/cache"), Some("/home/runner")),
            "/nas/cache"
        );
    }

    #[test]
    fn the_home_directory_carries_the_documented_default() {
        assert_eq!(
            cache_root_with(None, Some("/home/runner")),
            "/home/runner/.cache/gitforge"
        );
    }

    #[test]
    fn a_blank_override_or_home_falls_through() {
        assert_eq!(
            cache_root_with(Some("   "), Some("/home/runner")),
            "/home/runner/.cache/gitforge"
        );
        assert_eq!(cache_root_with(None, Some("")), DEFAULT_CACHE_ROOT);
    }

    #[test]
    fn without_an_override_or_a_home_the_system_path_applies() {
        assert_eq!(cache_root_with(None, None), DEFAULT_CACHE_ROOT);
    }
}

#[cfg(test)]
mod socket_tests {
    use super::*;

    #[test]
    fn the_daemon_socket_defaults_to_the_well_known_path() {
        assert_eq!(docker_socket_with(None), DEFAULT_DOCKER_SOCKET);
    }

    #[test]
    fn a_unix_docker_host_names_the_socket() {
        assert_eq!(
            docker_socket_with(Some("unix:///run/user/1000/podman/podman.sock")),
            "/run/user/1000/podman/podman.sock"
        );
    }

    #[test]
    fn a_tcp_docker_host_is_not_a_socket_path() {
        assert_eq!(
            docker_socket_with(Some("tcp://127.0.0.1:2375")),
            DEFAULT_DOCKER_SOCKET
        );
    }

    #[test]
    fn a_blank_docker_host_falls_through() {
        assert_eq!(docker_socket_with(Some("unix://")), DEFAULT_DOCKER_SOCKET);
        assert_eq!(docker_socket_with(Some("")), DEFAULT_DOCKER_SOCKET);
    }

    #[test]
    fn the_socket_placeholder_expands_to_the_daemon_socket() {
        let spec = VolumeSpec {
            volume_type: Some(VolumeType::Bind),
            source: Some("{docker_socket}".to_string()),
            destination: "/var/run/docker.sock".to_string(),
            read_only: true,
            size: None,
        };
        assert_eq!(
            spec.expanded_source(
                "dsc-ci-rust:16",
                "/var/cache/gitforge",
                "/run/user/1000/podman/podman.sock"
            )
            .as_deref(),
            Some("/run/user/1000/podman/podman.sock")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_socket_source_is_not_a_directory() {
        let root =
            std::env::temp_dir().join(format!("gitforge-socket-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("a scratch directory");
        let socket = root.join("docker.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("a unix socket");

        assert!(is_socket_source(&socket.to_string_lossy()));
        assert!(!is_socket_source(&root.to_string_lossy()));

        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn expands_cache_root_placeholder() {
        let spec = VolumeSpec {
            volume_type: Some(VolumeType::Bind),
            source: Some("{cache_root}/cargo/{image}".to_string()),
            destination: "/cargo-target".to_string(),
            read_only: false,
            size: None,
        };
        assert_eq!(
            spec.expanded_source(
                "dsc-ci-rust:10",
                "/var/cache/gitforge",
                "/var/run/docker.sock"
            )
            .as_deref(),
            Some("/var/cache/gitforge/cargo/dsc-ci-rust-10")
        );
    }

    #[test]
    fn job_cache_defaults_to_cargo_target_dir() {
        let cache = JobCache::default();
        assert_eq!(cache.source, "{cache_root}/cargo/{image}");
        assert_eq!(cache.mount_path, "/cargo-target");
        assert_eq!(cache.env_var, "CARGO_TARGET_DIR");
        let volume = cache.to_volume();
        assert_eq!(volume.resolved_type(), VolumeType::Bind);
        assert_eq!(volume.destination, "/cargo-target");
    }
}
