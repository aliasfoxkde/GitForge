//! GitForce Runner Agent
//!
//! Main entry point for the runner agent service.

use gitforge_process::{create_shutdown_flag, spawn_shutdown_handler, wait_for_shutdown};
use gitforge_runner::RunnerAgent;
#[allow(unused_imports)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("starting GitForce Runner Agent");

    // Initialize subreaper support without a global waitpid loop. Child
    // ownership must remain with the runtime that spawned it.
    if let Err(e) = gitforge_process::init_without_sigchld_reaper() {
        tracing::warn!("failed to initialize process supervision: {}", e);
    }

    // Load runner configuration from environment.
    // Fails fast at startup with an actionable error if required variables are
    // missing or invalid, rather than silently falling back to defaults.
    let config = gitforge_runner::RunnerConfig::from_env()
        .map_err(|e| anyhow::anyhow!("failed to load runner configuration: {}", e))?;

    // Create runner agent
    let mut agent = RunnerAgent::new(config).await?;

    // Register with scheduler
    let runner_id = agent.register().await?;
    tracing::info!("runner registered with ID: {}", runner_id);

    tracing::info!("Runner Agent initialized successfully");

    // Set up shutdown handling
    let shutdown = create_shutdown_flag();
    let shutdown_flag = shutdown.clone();

    // Spawn graceful shutdown handler
    spawn_shutdown_handler(shutdown_flag);

    // Start the registered agent's heartbeat and job-fetch loops. The agent
    // owns its executor; keep the loop under a task handle so shutdown can
    // stop it cleanly and propagate runtime failures to the service.
    let agent_loop = agent.clone();
    let runner_task = tokio::spawn(async move { agent_loop.run().await });

    // Wait for shutdown signal
    let shutdown_future = create_shutdown_future(shutdown.clone());
    tracing::info!("Runner Agent running, press Ctrl+C to stop");

    // Wait for shutdown signal
    timeout(Duration::MAX, shutdown_future).await.ok();

    tracing::info!("shutting down Runner Agent");

    // Stop the agent gracefully (force=false to wait for jobs)
    agent.stop(false).await;

    // Wait for active jobs to complete with a timeout
    const JOB_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
    if !agent.wait_for_jobs_complete(JOB_SHUTDOWN_TIMEOUT).await {
        tracing::warn!("jobs did not complete in time, force cancelling");
        agent.stop(true).await;
    }

    runner_task
        .await
        .map_err(|e| anyhow::anyhow!("runner task join failed: {}", e))??;

    // Graceful shutdown delay
    graceful_shutdown_delay().await;

    tracing::info!("Runner Agent stopped");
    Ok(())
}

/// Create the shutdown future that waits for shutdown signal
pub async fn create_shutdown_future(shutdown: Arc<AtomicBool>) {
    wait_for_shutdown(shutdown).await;
}

/// Perform graceful shutdown delay
pub async fn graceful_shutdown_delay() {
    timeout(Duration::from_secs(2), async {
        tokio::time::sleep(Duration::from_secs(1)).await;
    })
    .await
    .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_create_shutdown_flag_initial_state() {
        let flag = create_shutdown_flag();
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_create_shutdown_flag_clone() {
        let flag1 = create_shutdown_flag();
        let flag2 = flag1.clone();
        flag1.store(true, Ordering::SeqCst);
        assert!(flag2.load(Ordering::SeqCst));
    }

    #[test]
    fn test_graceful_shutdown_delay_does_not_panic() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                graceful_shutdown_delay().await;
            });
    }

    #[tokio::test]
    async fn test_create_shutdown_future() {
        let shutdown = create_shutdown_flag();
        let shutdown_flag = shutdown.clone();

        // Set shutdown after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            shutdown_flag.store(true, Ordering::SeqCst);
        });

        create_shutdown_future(shutdown).await;
    }

    #[tokio::test]
    async fn test_spawn_shutdown_handler_does_not_panic() {
        let flag = create_shutdown_flag();
        // Just verify the function doesn't panic when called
        spawn_shutdown_handler(flag);
    }

    #[test]
    fn test_shutdown_flag_is_atomic() {
        let flag = create_shutdown_flag();
        // Verify atomic operations work
        assert!(!flag.load(Ordering::SeqCst));
        flag.store(true, Ordering::SeqCst);
        assert!(flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_shutdown_flag_load_ordering() {
        // Verify SeqCst ordering is used
        let flag = create_shutdown_flag();
        let value = flag.load(Ordering::SeqCst);
        assert!(!value);
    }

    #[test]
    fn test_graceful_shutdown_delay_completes() {
        // Test that the delay actually waits
        let start = std::time::Instant::now();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                graceful_shutdown_delay().await;
            });
        // Should have taken at least 1 second
        assert!(start.elapsed().as_secs() >= 1);
    }

    #[test]
    fn test_runner_service_config_from_env_success() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = temp_env::with_vars([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_RUNNER_NAME", Some("test-runner")),
            ("GITFORGE_RUNNER_CAPACITY", Some("3")),
            ("GITFORGE_HEARTBEAT_INTERVAL", Some("45")),
            ("GITFORGE_FETCH_INTERVAL", Some("8")),
            ("GITFORGE_SCHEDULER_TOKEN", None::<&str>),
        ]);
        let config = gitforge_runner::RunnerConfig::from_env().expect("valid env should parse");
        assert_eq!(config.scheduler_url, "http://localhost:42781");
        assert_eq!(config.name, "test-runner");
        assert_eq!(config.capacity, 3);
        assert_eq!(config.heartbeat_interval_secs, 45);
        assert_eq!(config.fetch_interval_secs, 8);
        assert!(config.scheduler_token.is_none());
    }

    #[test]
    fn test_runner_service_config_missing_scheduler_url() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = temp_env::with_vars([("GITFORGE_SCHEDULER_URL", None::<&str>)]);
        let result = gitforge_runner::RunnerConfig::from_env();
        assert!(
            result.is_err(),
            "missing GITFORGE_SCHEDULER_URL should fail"
        );
        let err = result.unwrap_err();
        assert!(err.to_string().contains("GITFORGE_SCHEDULER_URL"));
    }

    mod temp_env {
        //! Minimal env-isolation for tests.
        pub struct TempVars {
            _guard: Vec<(String, Option<String>)>,
        }

        pub fn with_vars<I, K, V>(vars: I) -> TempVars
        where
            I: IntoIterator<Item = (K, Option<V>)>,
            K: AsRef<str>,
            V: AsRef<str>,
        {
            let mut guards = Vec::new();
            for (key, value) in vars {
                let key = key.as_ref().to_string();
                let value = value.as_ref().map(|v| v.as_ref().to_string());
                let prev = std::env::var(&key).ok();
                if let Some(ref v) = value {
                    std::env::set_var(&key, v);
                } else {
                    std::env::remove_var(&key);
                }
                guards.push((key, prev));
            }
            TempVars { _guard: guards }
        }

        impl Drop for TempVars {
            fn drop(&mut self) {
                for (key, value) in self._guard.iter().rev() {
                    match value {
                        Some(v) => std::env::set_var(key, v),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }
}
