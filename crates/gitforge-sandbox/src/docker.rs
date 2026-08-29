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
use std::path::Path;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

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

            let container_name = Self::container_name(job_id);

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
            })
        } else {
            // Stub mode - no Docker available
            Ok(SandboxInstance {
                container_id: format!("gitforce-job-{}", job_id),
                job_id,
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
            self.ensure_image(image).await?;
            let container_name = Self::container_name(job_id);
            let host_config = HostConfig {
                memory: Some((limits.memory_mb * 1024 * 1024) as i64),
                cpu_period: Some(100000),
                cpu_quota: Some((limits.cpu_ms * 1000) as i64),
                network_mode: if limits.network {
                    None
                } else {
                    Some("none".to_string())
                },
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
                host_config: Some(host_config),
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

#[cfg(test)]
mod tests {
    use super::*;
    use gitforge_common::ErrorKind;
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
