//! CLI integration tests
//!
//! Tests for CLI argument parsing and command handling.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "gitforge")]
#[command(version = "0.1.0")]
struct TestCli {
    #[arg(short, long)]
    verbose: bool,
    #[arg(short, long)]
    server: Option<String>,
    #[arg(short, long)]
    token: Option<String>,
    #[command(subcommand)]
    command: TestCommands,
}

#[derive(clap::Subcommand, Debug)]
enum TestCommands {
    /// Authentication commands
    Auth {
        #[arg(long)]
        login: Option<String>,
        #[arg(long)]
        logout: bool,
        #[arg(long)]
        status: bool,
        #[arg(long)]
        whoami: bool,
    },
    /// Repository commands
    Repo {
        #[arg(short, long)]
        list: bool,
        #[arg(short, long)]
        create: Option<String>,
        #[arg(short, long)]
        info: Option<String>,
        #[arg(short, long)]
        delete: Option<String>,
        #[arg(long)]
        clone: Option<String>,
        #[arg(long)]
        init: Option<String>,
    },
    /// Git operations
    Git {
        #[arg(long)]
        init: Option<String>,
        #[arg(long)]
        clone: Option<String>,
        #[arg(long)]
        status: bool,
        #[arg(long)]
        push: bool,
        #[arg(long)]
        pull: bool,
        #[arg(long)]
        add: Option<String>,
        #[arg(long)]
        commit: Option<String>,
        #[arg(long)]
        log: bool,
        #[arg(long)]
        remote: bool,
    },
    /// Pipeline commands
    Pipeline {
        #[arg(short, long)]
        list: bool,
        #[arg(short, long)]
        show: Option<String>,
        #[arg(short, long)]
        run: Option<String>,
        #[arg(short, long)]
        watch: Option<String>,
        #[arg(long)]
        create: Option<String>,
        #[arg(long)]
        delete: Option<String>,
    },
    /// Runner commands
    Runner {
        #[arg(short, long)]
        list: bool,
        #[arg(short, long)]
        info: Option<String>,
        #[arg(short, long)]
        register: Option<String>,
        #[arg(long)]
        capacity: Option<i32>,
        #[arg(long)]
        deregister: Option<String>,
    },
    /// Sync commands
    Sync {
        #[arg(long)]
        status: bool,
        #[arg(long)]
        push: bool,
        #[arg(long)]
        pull: bool,
        #[arg(long)]
        init: Option<String>,
    },
    /// AI code review
    Review {
        #[arg(long)]
        staged: bool,
        #[arg(long)]
        diff: bool,
        #[arg(long)]
        diff_content: Option<String>,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        context: Option<String>,
        #[arg(long, default_value = "anthropic")]
        provider: String,
        #[arg(short, long)]
        verbose: bool,
    },
}

#[test]
fn test_cli_auth_login() {
    let cli =
        TestCli::try_parse_from(["gitforge", "--verbose", "auth", "--login", "testuser"]).unwrap();
    assert!(cli.verbose);
    match cli.command {
        TestCommands::Auth { login, .. } => {
            assert_eq!(login, Some("testuser".to_string()));
        }
        _ => panic!("Expected Auth command"),
    }
}

#[test]
fn test_cli_auth_logout() {
    let cli = TestCli::try_parse_from(["gitforge", "auth", "--logout"]).unwrap();
    match cli.command {
        TestCommands::Auth { logout, .. } => {
            assert!(logout);
        }
        _ => panic!("Expected Auth command"),
    }
}

#[test]
fn test_cli_auth_status() {
    let cli = TestCli::try_parse_from(["gitforge", "auth", "--status"]).unwrap();
    match cli.command {
        TestCommands::Auth { status, .. } => {
            assert!(status);
        }
        _ => panic!("Expected Auth command"),
    }
}

#[test]
fn test_cli_auth_whoami() {
    let cli = TestCli::try_parse_from(["gitforge", "auth", "--whoami"]).unwrap();
    match cli.command {
        TestCommands::Auth { whoami, .. } => {
            assert!(whoami);
        }
        _ => panic!("Expected Auth command"),
    }
}

#[test]
fn test_cli_repo_list() {
    let cli = TestCli::try_parse_from(["gitforge", "repo", "--list"]).unwrap();
    match cli.command {
        TestCommands::Repo { list, .. } => {
            assert!(list);
        }
        _ => panic!("Expected Repo command"),
    }
}

#[test]
fn test_cli_repo_create() {
    let cli = TestCli::try_parse_from(["gitforge", "repo", "--create", "my-repo"]).unwrap();
    match cli.command {
        TestCommands::Repo { create, .. } => {
            assert_eq!(create, Some("my-repo".to_string()));
        }
        _ => panic!("Expected Repo command"),
    }
}

#[test]
fn test_cli_repo_info() {
    let cli = TestCli::try_parse_from(["gitforge", "repo", "--info", "test-repo"]).unwrap();
    match cli.command {
        TestCommands::Repo { info, .. } => {
            assert_eq!(info, Some("test-repo".to_string()));
        }
        _ => panic!("Expected Repo command"),
    }
}

#[test]
fn test_cli_repo_delete() {
    let cli = TestCli::try_parse_from(["gitforge", "repo", "--delete", "old-repo"]).unwrap();
    match cli.command {
        TestCommands::Repo { delete, .. } => {
            assert_eq!(delete, Some("old-repo".to_string()));
        }
        _ => panic!("Expected Repo command"),
    }
}

#[test]
fn test_cli_repo_clone() {
    let cli = TestCli::try_parse_from([
        "gitforge",
        "repo",
        "--clone",
        "git@github.com:user/repo.git",
    ])
    .unwrap();
    match cli.command {
        TestCommands::Repo { clone, .. } => {
            assert_eq!(clone, Some("git@github.com:user/repo.git".to_string()));
        }
        _ => panic!("Expected Repo command"),
    }
}

#[test]
fn test_cli_repo_init() {
    let cli = TestCli::try_parse_from(["gitforge", "repo", "--init", "/path/to/repo"]).unwrap();
    match cli.command {
        TestCommands::Repo { init, .. } => {
            assert_eq!(init, Some("/path/to/repo".to_string()));
        }
        _ => panic!("Expected Repo command"),
    }
}

#[test]
fn test_cli_pipeline_list() {
    let cli = TestCli::try_parse_from(["gitforge", "pipeline", "--list"]).unwrap();
    match cli.command {
        TestCommands::Pipeline { list, .. } => {
            assert!(list);
        }
        _ => panic!("Expected Pipeline command"),
    }
}

#[test]
fn test_cli_pipeline_show() {
    let cli = TestCli::try_parse_from(["gitforge", "pipeline", "--show", "abc123"]).unwrap();
    match cli.command {
        TestCommands::Pipeline { show, .. } => {
            assert_eq!(show, Some("abc123".to_string()));
        }
        _ => panic!("Expected Pipeline command"),
    }
}

#[test]
fn test_cli_pipeline_run() {
    let cli = TestCli::try_parse_from(["gitforge", "pipeline", "--run", "pipeline-xyz"]).unwrap();
    match cli.command {
        TestCommands::Pipeline { run, .. } => {
            assert_eq!(run, Some("pipeline-xyz".to_string()));
        }
        _ => panic!("Expected Pipeline command"),
    }
}

#[test]
fn test_cli_pipeline_watch() {
    let cli = TestCli::try_parse_from(["gitforge", "pipeline", "--watch", "run-123"]).unwrap();
    match cli.command {
        TestCommands::Pipeline { watch, .. } => {
            assert_eq!(watch, Some("run-123".to_string()));
        }
        _ => panic!("Expected Pipeline command"),
    }
}

#[test]
fn test_cli_pipeline_create() {
    let cli = TestCli::try_parse_from(["gitforge", "pipeline", "--create", "my-pipeline"]).unwrap();
    match cli.command {
        TestCommands::Pipeline { create, .. } => {
            assert_eq!(create, Some("my-pipeline".to_string()));
        }
        _ => panic!("Expected Pipeline command"),
    }
}

#[test]
fn test_cli_pipeline_delete() {
    let cli =
        TestCli::try_parse_from(["gitforge", "pipeline", "--delete", "old-pipeline"]).unwrap();
    match cli.command {
        TestCommands::Pipeline { delete, .. } => {
            assert_eq!(delete, Some("old-pipeline".to_string()));
        }
        _ => panic!("Expected Pipeline command"),
    }
}

#[test]
fn test_cli_runner_list() {
    let cli = TestCli::try_parse_from(["gitforge", "runner", "--list"]).unwrap();
    match cli.command {
        TestCommands::Runner { list, .. } => {
            assert!(list);
        }
        _ => panic!("Expected Runner command"),
    }
}

#[test]
fn test_cli_runner_register() {
    let cli = TestCli::try_parse_from(["gitforge", "runner", "--register", "my-runner"]).unwrap();
    match cli.command {
        TestCommands::Runner { register, .. } => {
            assert_eq!(register, Some("my-runner".to_string()));
        }
        _ => panic!("Expected Runner command"),
    }
}

#[test]
fn test_cli_runner_register_with_capacity() {
    let cli = TestCli::try_parse_from([
        "gitforge",
        "runner",
        "--register",
        "big-runner",
        "--capacity",
        "8",
    ])
    .unwrap();
    match cli.command {
        TestCommands::Runner {
            register, capacity, ..
        } => {
            assert_eq!(register, Some("big-runner".to_string()));
            assert_eq!(capacity, Some(8));
        }
        _ => panic!("Expected Runner command"),
    }
}

#[test]
fn test_cli_runner_info() {
    let cli = TestCli::try_parse_from(["gitforge", "runner", "--info", "runner-1"]).unwrap();
    match cli.command {
        TestCommands::Runner { info, .. } => {
            assert_eq!(info, Some("runner-1".to_string()));
        }
        _ => panic!("Expected Runner command"),
    }
}

#[test]
fn test_cli_runner_deregister() {
    let cli =
        TestCli::try_parse_from(["gitforge", "runner", "--deregister", "runner-old"]).unwrap();
    match cli.command {
        TestCommands::Runner { deregister, .. } => {
            assert_eq!(deregister, Some("runner-old".to_string()));
        }
        _ => panic!("Expected Runner command"),
    }
}

#[test]
fn test_cli_sync_status() {
    let cli = TestCli::try_parse_from(["gitforge", "sync", "--status"]).unwrap();
    match cli.command {
        TestCommands::Sync { status, .. } => {
            assert!(status);
        }
        _ => panic!("Expected Sync command"),
    }
}

#[test]
fn test_cli_sync_init() {
    let cli = TestCli::try_parse_from(["gitforge", "sync", "--init", "/path/to/dir"]).unwrap();
    match cli.command {
        TestCommands::Sync { init, .. } => {
            assert_eq!(init, Some("/path/to/dir".to_string()));
        }
        _ => panic!("Expected Sync command"),
    }
}

#[test]
fn test_cli_sync_push() {
    let cli = TestCli::try_parse_from(["gitforge", "sync", "--push"]).unwrap();
    match cli.command {
        TestCommands::Sync { push, .. } => {
            assert!(push);
        }
        _ => panic!("Expected Sync command"),
    }
}

#[test]
fn test_cli_sync_pull() {
    let cli = TestCli::try_parse_from(["gitforge", "sync", "--pull"]).unwrap();
    match cli.command {
        TestCommands::Sync { pull, .. } => {
            assert!(pull);
        }
        _ => panic!("Expected Sync command"),
    }
}

#[test]
fn test_cli_git_init() {
    let cli = TestCli::try_parse_from(["gitforge", "git", "--init", "/path/to/repo"]).unwrap();
    match cli.command {
        TestCommands::Git { init, .. } => {
            assert_eq!(init, Some("/path/to/repo".to_string()));
        }
        _ => panic!("Expected Git command"),
    }
}

#[test]
fn test_cli_git_clone() {
    let cli =
        TestCli::try_parse_from(["gitforge", "git", "--clone", "git@github.com:user/repo.git"])
            .unwrap();
    match cli.command {
        TestCommands::Git { clone, .. } => {
            assert_eq!(clone, Some("git@github.com:user/repo.git".to_string()));
        }
        _ => panic!("Expected Git command"),
    }
}

#[test]
fn test_cli_git_status() {
    let cli = TestCli::try_parse_from(["gitforge", "git", "--status"]).unwrap();
    match cli.command {
        TestCommands::Git { status, .. } => {
            assert!(status);
        }
        _ => panic!("Expected Git command"),
    }
}

#[test]
fn test_cli_git_push() {
    let cli = TestCli::try_parse_from(["gitforge", "git", "--push"]).unwrap();
    match cli.command {
        TestCommands::Git { push, .. } => {
            assert!(push);
        }
        _ => panic!("Expected Git command"),
    }
}

#[test]
fn test_cli_git_pull() {
    let cli = TestCli::try_parse_from(["gitforge", "git", "--pull"]).unwrap();
    match cli.command {
        TestCommands::Git { pull, .. } => {
            assert!(pull);
        }
        _ => panic!("Expected Git command"),
    }
}

#[test]
fn test_cli_git_add() {
    let cli = TestCli::try_parse_from(["gitforge", "git", "--add", "."]).unwrap();
    match cli.command {
        TestCommands::Git { add, .. } => {
            assert_eq!(add, Some(".".to_string()));
        }
        _ => panic!("Expected Git command"),
    }
}

#[test]
fn test_cli_git_commit() {
    let cli = TestCli::try_parse_from(["gitforge", "git", "--commit", "Initial commit"]).unwrap();
    match cli.command {
        TestCommands::Git { commit, .. } => {
            assert_eq!(commit, Some("Initial commit".to_string()));
        }
        _ => panic!("Expected Git command"),
    }
}

#[test]
fn test_cli_git_log() {
    let cli = TestCli::try_parse_from(["gitforge", "git", "--log"]).unwrap();
    match cli.command {
        TestCommands::Git { log, .. } => {
            assert!(log);
        }
        _ => panic!("Expected Git command"),
    }
}

#[test]
fn test_cli_git_remote() {
    let cli = TestCli::try_parse_from(["gitforge", "git", "--remote"]).unwrap();
    match cli.command {
        TestCommands::Git { remote, .. } => {
            assert!(remote);
        }
        _ => panic!("Expected Git command"),
    }
}

#[test]
fn test_cli_verbose_flag() {
    let cli = TestCli::try_parse_from(["gitforge", "--verbose", "auth", "--status"]).unwrap();
    assert!(cli.verbose);
}

#[test]
fn test_cli_server_option() {
    let cli = TestCli::try_parse_from([
        "gitforge",
        "--server",
        "http://localhost:9090",
        "auth",
        "--status",
    ])
    .unwrap();
    assert_eq!(cli.server, Some("http://localhost:9090".to_string()));
}

#[test]
fn test_cli_token_option() {
    let cli = TestCli::try_parse_from(["gitforge", "--token", "secret-token", "auth", "--status"])
        .unwrap();
    assert_eq!(cli.token, Some("secret-token".to_string()));
}

#[test]
fn test_cli_all_options_combined() {
    let cli = TestCli::try_parse_from([
        "gitforge",
        "--verbose",
        "--server",
        "http://gitforge.local",
        "--token",
        "abc123",
        "auth",
        "--status",
    ])
    .unwrap();
    assert!(cli.verbose);
    assert_eq!(cli.server, Some("http://gitforge.local".to_string()));
    assert_eq!(cli.token, Some("abc123".to_string()));
}

#[test]
fn test_cli_unknown_subcommand_fails() {
    let result = TestCli::try_parse_from(["gitforge", "unknown", "--something"]);
    assert!(result.is_err());
}

#[test]
fn test_cli_mixed_short_and_long_flags() {
    let cli = TestCli::try_parse_from(["gitforge", "-v", "sync", "--status"]).unwrap();
    assert!(cli.verbose);
    match cli.command {
        TestCommands::Sync { status, .. } => {
            assert!(status);
        }
        _ => panic!("Expected Sync command"),
    }
}

// ─── Review command parsing ─────────────────────────────────────────────────

#[test]
fn test_cli_review_staged_default() {
    // `review --staged` (the default) requires no extra args
    let cli = TestCli::try_parse_from(["gitforge", "review", "--staged"]).unwrap();
    match cli.command {
        TestCommands::Review {
            staged,
            diff,
            diff_content,
            base,
            target,
            context,
            provider,
            verbose,
        } => {
            assert!(staged);
            assert!(!diff);
            assert_eq!(diff_content, None);
            assert_eq!(base, None);
            assert_eq!(target, None);
            assert_eq!(context, None);
            assert_eq!(provider, "anthropic");
            assert!(!verbose);
        }
        _ => panic!("Expected Review command"),
    }
}

#[test]
fn test_cli_review_with_base_and_target() {
    let cli = TestCli::try_parse_from([
        "gitforge",
        "review",
        "--base",
        "main",
        "--target",
        "feature-x",
    ])
    .unwrap();
    match cli.command {
        TestCommands::Review { base, target, .. } => {
            assert_eq!(base, Some("main".to_string()));
            assert_eq!(target, Some("feature-x".to_string()));
        }
        _ => panic!("Expected Review command"),
    }
}

#[test]
fn test_cli_review_with_context() {
    let cli = TestCli::try_parse_from([
        "gitforge",
        "review",
        "--staged",
        "--context",
        "PR #123: Add new feature",
    ])
    .unwrap();
    match cli.command {
        TestCommands::Review { context, .. } => {
            assert_eq!(context, Some("PR #123: Add new feature".to_string()));
        }
        _ => panic!("Expected Review command"),
    }
}

#[test]
fn test_cli_review_with_explicit_diff_flag() {
    // --diff requires --diff-content; this only tests that the flags parse
    let cli = TestCli::try_parse_from([
        "gitforge",
        "review",
        "--diff",
        "--diff-content",
        "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-fn old\n+fn new",
    ])
    .unwrap();
    match cli.command {
        TestCommands::Review {
            diff,
            diff_content,
            staged,
            ..
        } => {
            assert!(diff);
            // staged is false when only --diff is given (no --staged, --base, or --target)
            assert!(!staged);
            assert!(diff_content.is_some());
        }
        _ => panic!("Expected Review command"),
    }
}

#[test]
fn test_cli_review_provider_openai() {
    let cli = TestCli::try_parse_from(["gitforge", "review", "--staged", "--provider", "openai"])
        .unwrap();
    match cli.command {
        TestCommands::Review { provider, .. } => {
            assert_eq!(provider, "openai");
        }
        _ => panic!("Expected Review command"),
    }
}

#[test]
fn test_cli_review_provider_ollama() {
    let cli = TestCli::try_parse_from(["gitforge", "review", "--staged", "--provider", "ollama"])
        .unwrap();
    match cli.command {
        TestCommands::Review { provider, .. } => {
            assert_eq!(provider, "ollama");
        }
        _ => panic!("Expected Review command"),
    }
}

#[test]
fn test_cli_review_verbose_short_flag() {
    let cli = TestCli::try_parse_from(["gitforge", "-v", "review", "--staged"]).unwrap();
    assert!(cli.verbose);
    match cli.command {
        TestCommands::Review { verbose, .. } => {
            assert!(
                !verbose,
                "verbose should be false when only global -v is set"
            );
        }
        _ => panic!("Expected Review command"),
    }
}

#[test]
fn test_cli_review_verbose_local_flag() {
    let cli = TestCli::try_parse_from(["gitforge", "review", "--staged", "-v"]).unwrap();
    match cli.command {
        TestCommands::Review { verbose, .. } => {
            assert!(verbose);
        }
        _ => panic!("Expected Review command"),
    }
}

#[test]
fn test_cli_review_all_options() {
    let cli = TestCli::try_parse_from([
        "gitforge",
        "--verbose",
        "review",
        "--staged",
        "--base",
        "develop",
        "--target",
        "feat/new-option",
        "--context",
        "Implements option parsing",
        "--provider",
        "ollama",
        "--verbose",
    ])
    .unwrap();
    assert!(cli.verbose);
    match cli.command {
        TestCommands::Review {
            staged,
            base,
            target,
            context,
            provider,
            verbose,
            ..
        } => {
            assert!(staged);
            assert_eq!(base, Some("develop".to_string()));
            assert_eq!(target, Some("feat/new-option".to_string()));
            assert_eq!(context, Some("Implements option parsing".to_string()));
            assert_eq!(provider, "ollama");
            assert!(verbose);
        }
        _ => panic!("Expected Review command"),
    }
}
