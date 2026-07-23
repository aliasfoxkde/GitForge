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
    },
    /// Repository commands
    Repo {
        #[arg(long)]
        list: bool,
        #[arg(long)]
        create: Option<String>,
        #[arg(long)]
        info: Option<String>,
        #[arg(long)]
        delete: Option<String>,
    },
    /// Pipeline commands
    Pipeline {
        #[arg(long)]
        list: bool,
        #[arg(long)]
        show: Option<String>,
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        watch: Option<String>,
    },
    /// Runner commands
    Runner {
        #[arg(long)]
        list: bool,
        #[arg(long)]
        info: Option<String>,
        #[arg(long)]
        register: Option<String>,
        #[arg(long)]
        capacity: Option<i32>,
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
}

#[test]
fn test_cli_auth_login() {
    let cli =
        TestCli::try_parse_from(["gitforge", "--verbose", "auth", "--login", "testuser"]).unwrap();
    assert!(cli.verbose);
    match cli.command {
        TestCommands::Auth {
            login,
            logout: _,
            status: _,
        } => {
            assert_eq!(login, Some("testuser".to_string()));
        }
        _ => panic!("Expected Auth command"),
    }
}

#[test]
fn test_cli_auth_logout() {
    let cli = TestCli::try_parse_from(["gitforge", "auth", "--logout"]).unwrap();
    match cli.command {
        TestCommands::Auth {
            login: _,
            logout,
            status: _,
        } => {
            assert!(logout);
        }
        _ => panic!("Expected Auth command"),
    }
}

#[test]
fn test_cli_auth_status() {
    let cli = TestCli::try_parse_from(["gitforge", "auth", "--status"]).unwrap();
    match cli.command {
        TestCommands::Auth {
            login: _,
            logout: _,
            status,
        } => {
            assert!(status);
        }
        _ => panic!("Expected Auth command"),
    }
}

#[test]
fn test_cli_repo_list() {
    let cli = TestCli::try_parse_from(["gitforge", "repo", "--list"]).unwrap();
    match cli.command {
        TestCommands::Repo {
            list,
            create: _,
            info: _,
            delete: _,
        } => {
            assert!(list);
        }
        _ => panic!("Expected Repo command"),
    }
}

#[test]
fn test_cli_repo_create() {
    let cli = TestCli::try_parse_from(["gitforge", "repo", "--create", "my-repo"]).unwrap();
    match cli.command {
        TestCommands::Repo {
            list: _,
            create,
            info: _,
            delete: _,
        } => {
            assert_eq!(create, Some("my-repo".to_string()));
        }
        _ => panic!("Expected Repo command"),
    }
}

#[test]
fn test_cli_repo_info() {
    let cli = TestCli::try_parse_from(["gitforge", "repo", "--info", "test-repo"]).unwrap();
    match cli.command {
        TestCommands::Repo {
            list: _,
            create: _,
            info,
            delete: _,
        } => {
            assert_eq!(info, Some("test-repo".to_string()));
        }
        _ => panic!("Expected Repo command"),
    }
}

#[test]
fn test_cli_repo_delete() {
    let cli = TestCli::try_parse_from(["gitforge", "repo", "--delete", "old-repo"]).unwrap();
    match cli.command {
        TestCommands::Repo {
            list: _,
            create: _,
            info: _,
            delete,
        } => {
            assert_eq!(delete, Some("old-repo".to_string()));
        }
        _ => panic!("Expected Repo command"),
    }
}

#[test]
fn test_cli_pipeline_list() {
    let cli = TestCli::try_parse_from(["gitforge", "pipeline", "--list"]).unwrap();
    match cli.command {
        TestCommands::Pipeline {
            list,
            show: _,
            run: _,
            watch: _,
        } => {
            assert!(list);
        }
        _ => panic!("Expected Pipeline command"),
    }
}

#[test]
fn test_cli_pipeline_show() {
    let cli = TestCli::try_parse_from(["gitforge", "pipeline", "--show", "abc123"]).unwrap();
    match cli.command {
        TestCommands::Pipeline {
            list: _,
            show,
            run: _,
            watch: _,
        } => {
            assert_eq!(show, Some("abc123".to_string()));
        }
        _ => panic!("Expected Pipeline command"),
    }
}

#[test]
fn test_cli_pipeline_run() {
    let cli = TestCli::try_parse_from(["gitforge", "pipeline", "--run", "pipeline-xyz"]).unwrap();
    match cli.command {
        TestCommands::Pipeline {
            list: _,
            show: _,
            run,
            watch: _,
        } => {
            assert_eq!(run, Some("pipeline-xyz".to_string()));
        }
        _ => panic!("Expected Pipeline command"),
    }
}

#[test]
fn test_cli_runner_list() {
    let cli = TestCli::try_parse_from(["gitforge", "runner", "--list"]).unwrap();
    match cli.command {
        TestCommands::Runner {
            list,
            info: _,
            register: _,
            capacity: _,
        } => {
            assert!(list);
        }
        _ => panic!("Expected Runner command"),
    }
}

#[test]
fn test_cli_runner_register() {
    let cli = TestCli::try_parse_from(["gitforge", "runner", "--register", "my-runner"]).unwrap();
    match cli.command {
        TestCommands::Runner {
            list: _,
            info: _,
            register,
            capacity,
        } => {
            assert_eq!(register, Some("my-runner".to_string()));
            assert_eq!(capacity, None);
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
            list: _,
            info: _,
            register,
            capacity,
        } => {
            assert_eq!(register, Some("big-runner".to_string()));
            assert_eq!(capacity, Some(8));
        }
        _ => panic!("Expected Runner command"),
    }
}

#[test]
fn test_cli_sync_status() {
    let cli = TestCli::try_parse_from(["gitforge", "sync", "--status"]).unwrap();
    match cli.command {
        TestCommands::Sync {
            status,
            push: _,
            pull: _,
            init: _,
        } => {
            assert!(status);
        }
        _ => panic!("Expected Sync command"),
    }
}

#[test]
fn test_cli_sync_init() {
    let cli = TestCli::try_parse_from(["gitforge", "sync", "--init", "/path/to/dir"]).unwrap();
    match cli.command {
        TestCommands::Sync {
            status: _,
            push: _,
            pull: _,
            init,
        } => {
            assert_eq!(init, Some("/path/to/dir".to_string()));
        }
        _ => panic!("Expected Sync command"),
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
fn test_cli_unknown_subcommand_fails() {
    let result = TestCli::try_parse_from(["gitforge", "unknown", "--something"]);
    assert!(result.is_err());
}
