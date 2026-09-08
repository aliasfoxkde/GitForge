//! SSH Git protocol helpers
//!
//! The SSH transport itself is served by the git-server service
//! (`services/git-server/src/ssh_server.rs`), which pipes authenticated SSH
//! channels to real `git upload-pack`/`git receive-pack` child processes.
//! This module keeps the command-parsing shared by that transport.

/// Extract command from SSH original_command
pub fn parse_ssh_command(cmd: &str) -> Option<(&str, &str)> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.len() >= 2 {
        let command = parts[0];
        let repo_path = parts[1];
        Some((command, repo_path))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ssh_command() {
        assert_eq!(
            parse_ssh_command("git-upload-pack /owner/repo.git"),
            Some(("git-upload-pack", "/owner/repo.git"))
        );
        assert_eq!(
            parse_ssh_command("git-receive-pack owner/repo"),
            Some(("git-receive-pack", "owner/repo"))
        );
    }

    #[test]
    fn test_parse_ssh_command_edge_cases() {
        // Single word
        assert_eq!(parse_ssh_command("git-upload-pack"), None);
        // Empty string
        assert_eq!(parse_ssh_command(""), None);
        // Extra whitespace
        assert_eq!(
            parse_ssh_command("git-upload-pack   /repo"),
            Some(("git-upload-pack", "/repo"))
        );
        // Multiple spaces between
        assert_eq!(
            parse_ssh_command("git-receive-pack  owner/repo"),
            Some(("git-receive-pack", "owner/repo"))
        );
        // Deep path
        assert_eq!(
            parse_ssh_command("git-upload-pack /owner/repo/path/to/refs"),
            Some(("git-upload-pack", "/owner/repo/path/to/refs"))
        );
    }

    #[test]
    fn test_parse_ssh_command_with_special_chars() {
        // Dot in repo name
        assert_eq!(
            parse_ssh_command("git-upload-pack /repo.with.dots.git"),
            Some(("git-upload-pack", "/repo.with.dots.git"))
        );
        // Underscore and hyphen
        assert_eq!(
            parse_ssh_command("git-upload-pack /my_repo-name"),
            Some(("git-upload-pack", "/my_repo-name"))
        );
    }

    #[test]
    fn test_parse_ssh_command_only_whitespace() {
        assert_eq!(parse_ssh_command("   "), None);
    }

    #[test]
    fn test_parse_ssh_command_with_leading_whitespace() {
        // Command with leading whitespace
        assert_eq!(
            parse_ssh_command("  git-upload-pack /repo"),
            Some(("git-upload-pack", "/repo"))
        );
    }

    #[test]
    fn test_parse_ssh_command_multiple_words_after_repo() {
        // Command with extra args after repo path
        assert_eq!(
            parse_ssh_command("git-upload-pack /repo.git extra args"),
            Some(("git-upload-pack", "/repo.git"))
        );
    }
}
