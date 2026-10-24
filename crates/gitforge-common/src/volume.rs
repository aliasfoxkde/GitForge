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

    /// The host source with the `{image}` and `{cache_root}` placeholders
    /// expanded against the job's image and the runner's cache root.
    pub fn expanded_source(&self, image: &str, cache_root: &str) -> Option<String> {
        self.source.as_ref().map(|source| {
            source
                .replace("{image}", &image_cache_key(image))
                .replace("{cache_root}", cache_root)
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
            spec.expanded_source("dsc-ci-rust:10", "/var/cache/gitforge")
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
            spec.expanded_source("dsc-ci-rust:10", "/var/cache/gitforge")
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
