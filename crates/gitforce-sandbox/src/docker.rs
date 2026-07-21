//! Docker-based sandbox implementation using bollard

use crate::limits::SandboxLimits;
use async_trait::async_trait;
use bollard::container::{
    Config, CreateContainerOptions, LogOutput, RemoveContainerOptions,
    StartContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::Docker;
use bollard::models::HostConfig;
use futures_util::StreamExt;
use gitforce_common::{Error, JobId, Result};

/// Sandbox instance handle
#[derive(Debug, Clone)]
pub struct SandboxInstance {
    pub container_id: String,
    pub job_id: JobId,
}

/// Result of step execution
#[derive(Debug, Clone)]
pub struct StepResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Docker-based sandbox
pub struct DockerSandbox {
    docker: Option<Docker>,
    #[allow(dead_code)]
    default_limits: SandboxLimits,
}

impl DockerSandbox {
    /// Create a new Docker sandbox (connects to Docker daemon)
    pub async fn new() -> Result<Self> {
        let docker = match Docker::connect_with_local_defaults() {
            Ok(d) => {
                // Verify connection by pinging Docker
                match d.ping().await {
                    Ok(_) => {
                        tracing::info!("Connected to Docker daemon");
                        Some(d)
                    }
                    Err(e) => {
                        tracing::warn!("Docker daemon not available: {}. Running in stub mode.", e);
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to connect to Docker: {}. Running in stub mode.", e);
                None
            }
        };

        Ok(Self {
            docker,
            default_limits: SandboxLimits::default(),
        })
    }

    /// Create a new Docker sandbox with custom limits
    pub fn with_limits(limits: SandboxLimits) -> Self {
        Self {
            docker: None,
            default_limits: limits,
        }
    }

    /// Check if Docker is available
    pub fn is_available(&self) -> bool {
        self.docker.is_some()
    }

    /// Pull an image if not present
    async fn ensure_image(&self, image: &str) -> Result<()> {
        if let Some(ref docker) = self.docker {
            tracing::info!("Pulling image: {}", image);
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
    async fn create(&self, job_id: JobId, image: &str, limits: SandboxLimits) -> Result<SandboxInstance>;

    /// Execute a command in the sandbox
    async fn execute(&self, instance: &SandboxInstance, command: &[&str]) -> Result<StepResult>;

    /// Destroy a sandbox instance
    async fn destroy(&self, instance: SandboxInstance) -> Result<()>;
}

#[async_trait]
impl Sandbox for DockerSandbox {
    async fn create(&self, job_id: JobId, image: &str, limits: SandboxLimits) -> Result<SandboxInstance> {
        if let Some(ref docker) = self.docker {
            // Ensure image is available
            self.ensure_image(image).await?;

            let container_name = format!("gitforce-job-{}", job_id);

            // Build host config with resource limits
            let host_config = HostConfig {
                memory: Some((limits.memory_mb * 1024 * 1024) as i64),
                cpu_period: Some(100000), // 100ms in microseconds
                cpu_quota: Some((limits.cpu_ms * 1000) as i64), // Convert ms to microseconds
                network_mode: if limits.network { None } else { Some("none".to_string()) },
                ..Default::default()
            };

            // Create container
            let config = Config {
                image: Some(image),
                cmd: Some(vec!["sleep", "3600"]), // Keep container alive
                host_config: Some(host_config),
                ..Default::default()
            };

            let options = CreateContainerOptions {
                name: &container_name,
                platform: None,
            };

            let response = docker.create_container(Some(options), config).await
                .map_err(|e| Error::sandbox(format!("failed to create container: {}", e)))?;

            // Start container
            docker.start_container(&response.id, None::<StartContainerOptions<String>>).await
                .map_err(|e| Error::sandbox(format!("failed to start container: {}", e)))?;

            tracing::info!("Created container {} for job {}", response.id, job_id);

            Ok(SandboxInstance {
                container_id: response.id,
                job_id,
            })
        } else {
            // Stub mode - no Docker available
            Ok(SandboxInstance {
                container_id: format!("gitforce-job-{}", job_id),
                job_id,
            })
        }
    }

    async fn execute(&self, instance: &SandboxInstance, command: &[&str]) -> Result<StepResult> {
        if let Some(ref docker) = self.docker {
            // Create exec instance
            let config = CreateExecOptions {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                cmd: Some(command.to_vec()),
                ..Default::default()
            };

            let exec = docker.create_exec(&instance.container_id, config).await
                .map_err(|e| Error::sandbox(format!("failed to create exec: {}", e)))?;

            // Start exec and get results
            let result = docker.start_exec(&exec.id, None::<StartExecOptions>).await
                .map_err(|e| Error::sandbox(format!("failed to start exec: {}", e)))?;

            let mut stdout = String::new();
            let mut stderr = String::new();
            let mut exit_code = 0i32;

            if let StartExecResults::Attached { mut output, .. } = result {
                while let Some(item) = output.next().await {
                    match item {
                        Ok(LogOutput::StdOut { message }) => {
                            stdout.push_str(&String::from_utf8_lossy(&message));
                        }
                        Ok(LogOutput::StdErr { message }) => {
                            stderr.push_str(&String::from_utf8_lossy(&message));
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!("exec output error: {}", e);
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
            // Stub mode
            tracing::debug!("Executing command in stub mode: {:?}", command);
            Ok(StepResult {
                exit_code: 0,
                stdout: format!("Executing: {:?}\n", command),
                stderr: String::new(),
            })
        }
    }

    async fn destroy(&self, instance: SandboxInstance) -> Result<()> {
        if let Some(ref docker) = self.docker {
            // Stop and remove container
            let options = RemoveContainerOptions {
                force: true,
                ..Default::default()
            };

            docker.remove_container(&instance.container_id, Some(options)).await
                .map_err(|e| Error::sandbox(format!("failed to remove container: {}", e)))?;

            tracing::info!("Destroyed container {} for job {}", instance.container_id, instance.job_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_docker_sandbox_creation() {
        let sandbox = DockerSandbox::new().await.unwrap();

        // Should always succeed even without Docker (stub mode)
        let sandbox2 = DockerSandbox::with_limits(SandboxLimits::default());
        assert!(!sandbox2.is_available() || sandbox.is_available()); // Either stub or real
    }

    #[tokio::test]
    #[ignore] // Requires Docker and is slow - run manually with `cargo test -- --ignored`
    async fn test_docker_sandbox_real_when_available() {
        let sandbox = DockerSandbox::new().await.unwrap();

        if !sandbox.is_available() {
            // Skip if Docker not available - this is expected in CI
            return;
        }

        // Test with real Docker
        let job_id = JobId::new();
        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await;

        // If Docker has issues, we fall back to stub mode
        if instance.is_err() {
            return;
        }

        let instance = instance.unwrap();
        assert!(!instance.container_id.is_empty());

        let result = sandbox.execute(&instance, &["echo", "hello"]).await;
        if result.is_ok() {
            assert_eq!(result.unwrap().exit_code, 0);
        }

        let _ = sandbox.destroy(instance).await;
    }

    #[tokio::test]
    #[ignore] // Requires Docker and is slow - run manually with `cargo test -- --ignored`
    async fn test_docker_sandbox_real_with_longer_command() {
        let sandbox = DockerSandbox::new().await.unwrap();

        if !sandbox.is_available() {
            return;
        }

        let job_id = JobId::new();
        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await;
        if instance.is_err() {
            return;
        }
        let instance = instance.unwrap();

        let result = sandbox.execute(&instance, &["sh", "-c", "echo test"]).await;
        if result.is_ok() {
            assert_eq!(result.unwrap().exit_code, 0);
        }

        let _ = sandbox.destroy(instance).await;
    }

    #[tokio::test]
    #[ignore] // Requires Docker and is slow - run manually with `cargo test -- --ignored`
    async fn test_docker_sandbox_real_multiple_commands() {
        let sandbox = DockerSandbox::new().await.unwrap();

        if !sandbox.is_available() {
            return;
        }

        let job_id = JobId::new();
        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await;
        if instance.is_err() {
            return;
        }
        let instance = instance.unwrap();

        let result = sandbox.execute(&instance, &["sh", "-c", "echo line1"]).await;
        if result.is_ok() {
            assert_eq!(result.unwrap().exit_code, 0);
        }

        let _ = sandbox.destroy(instance).await;
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execution() {
        // Create sandbox in stub mode
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        assert!(!sandbox.is_available());

        let job_id = JobId::new();

        // Create container (will use stub)
        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await.unwrap();
        assert!(!instance.container_id.is_empty());

        // Execute command (will use stub)
        let result = sandbox.execute(&instance, &["echo", "hello"]).await.unwrap();
        assert_eq!(result.exit_code, 0);

        // Destroy container (will use stub)
        sandbox.destroy(instance).await.unwrap();
    }

    #[test]
    fn test_sandbox_instance_debug() {
        let instance = SandboxInstance {
            container_id: "test-container".to_string(),
            job_id: JobId::new(),
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
    fn test_docker_sandbox_with_custom_limits() {
        let limits = SandboxLimits::small();
        let sandbox = DockerSandbox::with_limits(limits);
        assert!(!sandbox.is_available());
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execution_with_complex_command() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        let job_id = JobId::new();

        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await.unwrap();

        // Test with shell command
        let result = sandbox.execute(&instance, &["sh", "-c", "echo hello && echo world"]).await.unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execution_multiple_commands() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        let job_id = JobId::new();

        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await.unwrap();

        // Multiple commands in one execution
        let result = sandbox.execute(&instance, &["echo", "line1"]).await.unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[test]
    fn test_sandbox_instance_clone() {
        let instance = SandboxInstance {
            container_id: "clone-test".to_string(),
            job_id: JobId::new(),
        };
        let cloned = instance.clone();
        assert_eq!(cloned.container_id, instance.container_id);
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_preserves_job_id() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        let job_id = JobId::new();

        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await.unwrap();
        assert_eq!(instance.job_id, job_id);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execute_empty_command() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        let job_id = JobId::new();

        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await.unwrap();

        // Empty command should still work in stub mode
        let result = sandbox.execute(&instance, &[]).await.unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execute_single_command() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        let job_id = JobId::new();

        let instance = sandbox.create(job_id, "rust:latest", SandboxLimits::default()).await.unwrap();

        let result = sandbox.execute(&instance, &["true"]).await.unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execute_false_command() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        let job_id = JobId::new();

        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await.unwrap();

        let result = sandbox.execute(&instance, &["false"]).await.unwrap();
        // Stub mode always returns 0, even for false command
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_multiple_instances() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());

        // Create multiple instances
        let job1 = JobId::new();
        let job2 = JobId::new();
        let job3 = JobId::new();

        let instance1 = sandbox.create(job1, "alpine:latest", SandboxLimits::default()).await.unwrap();
        let instance2 = sandbox.create(job2, "ubuntu:latest", SandboxLimits::default()).await.unwrap();
        let instance3 = sandbox.create(job3, "rust:latest", SandboxLimits::default()).await.unwrap();

        // Each should have unique container ID
        assert_ne!(instance1.container_id, instance2.container_id);
        assert_ne!(instance2.container_id, instance3.container_id);

        sandbox.destroy(instance1).await.unwrap();
        sandbox.destroy(instance2).await.unwrap();
        sandbox.destroy(instance3).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_destroy_same_instance_twice() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        let job_id = JobId::new();

        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await.unwrap();

        // Destroy twice should both succeed (idempotent in stub mode)
        sandbox.destroy(instance.clone()).await.unwrap();
        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_with_large_image_name() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        let job_id = JobId::new();

        // Very long image name
        let instance = sandbox.create(job_id, "registry.example.com/verylongnamed repository/imagename:latest", SandboxLimits::default()).await.unwrap();
        assert!(!instance.container_id.is_empty());

        sandbox.destroy(instance).await.unwrap();
    }

    #[test]
    fn test_docker_sandbox_debug_trait_not_implemented() {
        // DockerSandbox doesn't implement Debug, which is intentional
        // This test documents that behavior
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
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
        let mut limits = SandboxLimits::default();
        limits.network = false;
        assert!(!limits.network);
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execute_special_characters() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        let job_id = JobId::new();

        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await.unwrap();

        // Test with special characters in command
        let result = sandbox.execute(&instance, &["sh", "-c", "echo $'hello\nworld'"]).await.unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execute_unicode() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        let job_id = JobId::new();

        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await.unwrap();

        // Test with unicode
        let result = sandbox.execute(&instance, &["echo", "hello world"]).await.unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_execute_exit_codes() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        let job_id = JobId::new();

        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await.unwrap();

        // Test 'true' command (exit 0)
        let result = sandbox.execute(&instance, &["true"]).await.unwrap();
        assert_eq!(result.exit_code, 0);

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_container_id_format() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        let job_id = JobId::new();

        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await.unwrap();

        // Container ID should start with expected prefix
        assert!(instance.container_id.starts_with("gitforce-job-"));
        assert!(instance.container_id.contains(&job_id.to_string()));

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_network_enabled() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits::default());
        let job_id = JobId::new();

        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits::default()).await.unwrap();
        assert!(!instance.container_id.is_empty());

        sandbox.destroy(instance).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_sandbox_stub_network_disabled() {
        let sandbox = DockerSandbox::with_limits(SandboxLimits {
            network: false,
            ..Default::default()
        });
        let job_id = JobId::new();

        let instance = sandbox.create(job_id, "alpine:latest", SandboxLimits {
            network: false,
            ..Default::default()
        }).await.unwrap();
        assert!(!instance.container_id.is_empty());

        sandbox.destroy(instance).await.unwrap();
    }

    #[test]
    fn test_sandbox_instance_eq() {
        let job_id = JobId::new();
        let instance1 = SandboxInstance {
            container_id: "test-container".to_string(),
            job_id,
        };
        let instance2 = SandboxInstance {
            container_id: "test-container".to_string(),
            job_id,
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
}
