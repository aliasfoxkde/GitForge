//! Classification of the scheduler's exclusive resource classes.
//!
//! A resource class marks work that must not overlap on one runner. The only
//! class today is `workspace-cargo-test`: a workspace-wide Cargo test builds
//! the whole workspace into one target tree, so two of them on one runner
//! fight over the same build artifacts.
//!
//! The class is decided by the cargo SUBCOMMAND the command actually runs.
//! The earlier classifier scanned the command TEXT for the words `cargo`,
//! `test` and `--workspace` independently, so any command that merely
//! MENTIONED those words landed in the class — a comment line, or another
//! cargo subcommand such as `cargo llvm-cov`. A job that was not a workspace
//! cargo test was then excluded from runners that were free to take it, and
//! the exclusion was invisible in the job's own evidence.
//!
//! The scan below is lexical and deliberately conservative: it reads the
//! subcommand of a cargo invocation and requires `--workspace` (or its
//! `--all` alias) among that invocation's arguments.
//!
//! TWO DIRECTIONS, CHOSEN DELIBERATELY.
//!
//! * A false POSITIVE (a job that is not a workspace cargo test, classified as
//!   one) costs a scheduling deferral. The deferral is recorded in the job's
//!   durable `deferred_reason`, so the cost is visible and bounded.
//! * A false NEGATIVE (a workspace cargo test that is not classified) lets
//!   two workspace cargo tests build the same target tree on one runner. That
//!   is the failure that killed the coverage jobs with ENOENT on the test
//!   binary on 2026-11-05, and it is silent.
//!
//! So the scan looks for a cargo invocation ANYWHERE in a segment, not only at
//! the segment's first program word. A wrapper program in front of the
//! invocation — `flock <lockfile> cargo test --workspace` — still runs that
//! invocation, and the wrapper is exactly what the older, looser classifier
//! used to catch. The residual imprecision is an unquoted textual mention of
//! a cargo test inside a segment; it costs a visible deferral, never a
//! corrupted build.

/// Cargo's global options that take a separate value argument. The value is
/// never the subcommand, so the subcommand scan steps over both.
const CARGO_VALUE_OPTIONS: &[&str] = &[
    "--color",
    "--config",
    "--manifest-path",
    "--target",
    "--target-dir",
    "-Z",
];

/// Whether the command runs `cargo test` over the whole workspace.
///
/// The command text is a shell script, so the scan looks for a segment that
/// runs a cargo invocation whose SUBCOMMAND is `test` and whose arguments
/// include `--workspace` (or its `--all` alias). Comment lines are skipped,
/// and the workspace flag must belong to the cargo test invocation itself, so
/// a mention of "cargo test --workspace" inside a comment or an echoed
/// string does not create the class.
pub fn workspace_cargo_test(command: &str) -> bool {
    logical_lines(command).iter().any(|line| {
        shell_segments(line)
            .iter()
            .any(|segment| segment_runs_workspace_cargo_test(segment))
    })
}

/// Join backslash-continued lines into the logical lines a shell would run.
fn logical_lines(command: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut pending = String::new();
    for raw in command.lines() {
        let trimmed = raw.trim_end();
        let continued = trimmed.ends_with('\\');
        let piece = if continued {
            &trimmed[..trimmed.len() - 1]
        } else {
            trimmed
        };
        pending.push_str(piece);
        pending.push(' ');
        if !continued {
            lines.push(std::mem::take(&mut pending));
        }
    }
    if !pending.trim().is_empty() {
        lines.push(pending);
    }
    lines
}

/// Split a logical line into the shell segments separated by `&&`, `||`, `;`
/// and `|`. A segment is one command's worth of words. Separators inside
/// quotes are not separators, and a `#` comment runs to the end of the line
/// without contributing a segment.
fn shell_segments(line: &str) -> Vec<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        if let Some(open) = quote {
            current.push(character);
            if character == open {
                quote = None;
            }
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
            current.push(character);
            continue;
        }
        if character == '#' && at_word_boundary(&current) {
            // A comment runs to the end of the line; its text is not a command.
            break;
        }
        let separator = match character {
            '&' | '|' if characters.peek() == Some(&character) => {
                characters.next();
                true
            }
            '|' | ';' => true,
            _ => false,
        };
        if separator {
            segments.push(std::mem::take(&mut current));
            continue;
        }
        current.push(character);
    }
    segments.push(current);
    segments
}

/// Whether the segment so far ends at a word boundary, where a `#` starts a
/// comment rather than continuing a word.
fn at_word_boundary(current: &str) -> bool {
    current.trim_end().is_empty()
}

/// Split a shell segment into words, honouring single and double quotes.
fn shell_words(segment: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut has_word = false;
    let mut quote: Option<char> = None;
    for character in segment.chars() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
            has_word = true;
            continue;
        }
        if character.is_whitespace() {
            if has_word {
                words.push(std::mem::take(&mut current));
                has_word = false;
            }
            continue;
        }
        current.push(character);
        has_word = true;
    }
    if has_word {
        words.push(current);
    }
    words
}

/// Whether one shell segment runs a workspace-wide cargo test.
///
/// Every word position is considered as the start of a cargo invocation, so a
/// wrapper program in front of the invocation does not hide it.
fn segment_runs_workspace_cargo_test(segment: &str) -> bool {
    let words = shell_words(segment);
    words.iter().enumerate().any(|(start, word)| {
        is_cargo_program(word) && cargo_invocation_is_workspace_test(&words, start)
    })
}

/// Whether the cargo invocation that starts at `start` is a workspace-wide
/// cargo test.
fn cargo_invocation_is_workspace_test(words: &[String], start: usize) -> bool {
    let mut index = start + 1;
    // Step over cargo's global options to reach the subcommand.
    while let Some(word) = words.get(index) {
        if word.starts_with('+') {
            // A toolchain override (`+nightly`) is not a subcommand.
            index += 1;
            continue;
        }
        if word.starts_with('-') {
            let takes_value = CARGO_VALUE_OPTIONS.contains(&word.as_str()) && !word.contains('=');
            index += if takes_value { 2 } else { 1 };
            continue;
        }
        break;
    }
    // The first bare word after cargo is the subcommand.
    let Some(subcommand) = words.get(index) else {
        return false;
    };
    if subcommand != "test" {
        return false;
    }
    // `--workspace` (or its `--all` alias) must be an argument of THIS
    // invocation, not of another command in the same script.
    words[index + 1..]
        .iter()
        .any(|argument| argument == "--workspace" || argument == "--all")
}

/// Whether a word is a `NAME=value` environment assignment.
fn is_env_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Whether a word invokes cargo.
fn is_cargo_program(word: &str) -> bool {
    // An environment assignment is a value, not a program word, even when its
    // value happens to end in `/cargo`.
    if is_env_assignment(word) {
        return false;
    }
    word == "cargo" || word.ends_with("/cargo")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DIL coverage job's command verbatim. The comment mentions "the
    /// test binary" and the invocation is `cargo llvm-cov --workspace`; the
    /// old word scan classified it as a workspace cargo test.
    const DIL_COVERAGE_COMMAND: &str = "command -v cargo-llvm-cov >/dev/null 2>&1 || { echo \"cargo-llvm-cov not in image; skipping\"; exit 0; }\nexport CARGO_BUILD_JOBS=4\n# GF-11 (no job-level lock any more - the runner serializes the\n# jobs that share this host cache directory; see the note below).\n# The runner holds that lock for the job's lifetime, so a job\n# that finds the directory busy waits for it instead of\n# compiling into a tree another job is rewriting.\n# The procedure that used to compensate for the defect - `flock\n# /cargo-target/.dil-cargo.lock` - was removed with the fix.\n# This is the job that died on 2026-11-05 with ENOENT on the\n# test binary; the runner's mount lock is what keeps the tree\n# it executes from being replaced under it.\ncargo llvm-cov --workspace --summary-only --fail-under-lines 95";

    #[test]
    fn a_real_workspace_cargo_test_is_classified() {
        assert!(workspace_cargo_test("cargo test --workspace"));
        assert!(workspace_cargo_test(
            "cargo test --workspace -- --test-threads=2"
        ));
        assert!(workspace_cargo_test(
            "export CARGO_BUILD_JOBS=4\ncargo test --workspace"
        ));
    }

    #[test]
    fn the_dil_coverage_command_is_not_a_workspace_cargo_test() {
        assert!(
            !workspace_cargo_test(DIL_COVERAGE_COMMAND),
            "cargo llvm-cov with a comment mentioning the test binary is not a cargo test"
        );
    }

    #[test]
    fn another_cargo_subcommand_is_not_a_workspace_cargo_test() {
        assert!(!workspace_cargo_test(
            "cargo llvm-cov --workspace --summary-only"
        ));
        assert!(!workspace_cargo_test(
            "cargo clippy --workspace --all-targets -- -D warnings"
        ));
        assert!(!workspace_cargo_test("cargo build --workspace"));
    }

    #[test]
    fn a_comment_mentioning_the_whole_invocation_is_not_classified() {
        assert!(!workspace_cargo_test(
            "# run cargo test --workspace later\ncargo fmt --all -- --check"
        ));
    }

    #[test]
    fn a_cargo_test_without_the_workspace_flag_is_not_classified() {
        assert!(!workspace_cargo_test("cargo test -p gitforge-scheduler"));
        assert!(!workspace_cargo_test("cargo test"));
    }

    #[test]
    fn an_echoed_invocation_is_not_classified() {
        assert!(!workspace_cargo_test("echo 'cargo test --workspace'"));
    }

    #[test]
    fn the_workspace_flag_must_belong_to_the_cargo_test_invocation() {
        assert!(!workspace_cargo_test(
            "cargo test -p foo && cargo build --workspace"
        ));
        assert!(workspace_cargo_test(
            "cargo build --workspace && cargo test --workspace"
        ));
    }

    #[test]
    fn a_per_package_test_is_not_a_workspace_cargo_test() {
        // The clippy invocation carries `--workspace`; the test invocation
        // does not. The old word scan classified this script because the two
        // words appeared anywhere in it.
        assert!(!workspace_cargo_test(
            "cargo clippy --locked --workspace --all-targets -- -D warnings\ncargo test --locked -p aegis-core --all-targets -- --test-threads=1"
        ));
    }

    #[test]
    fn cargo_global_options_do_not_hide_the_subcommand() {
        assert!(workspace_cargo_test(
            "cargo --manifest-path /workspace/Cargo.toml test --workspace"
        ));
        assert!(workspace_cargo_test(
            "cargo --color always test --workspace"
        ));
        assert!(workspace_cargo_test("cargo +nightly test --workspace"));
    }

    #[test]
    fn the_all_alias_is_the_workspace_flag() {
        assert!(workspace_cargo_test("cargo test --all"));
        assert!(!workspace_cargo_test("cargo test --all-features"));
    }

    #[test]
    fn an_env_prefixed_invocation_is_classified() {
        assert!(workspace_cargo_test(
            "CARGO_BUILD_JOBS=4 cargo test --workspace"
        ));
        assert!(workspace_cargo_test("env FOO=bar cargo test --workspace"));
    }

    #[test]
    fn a_backslash_continued_invocation_is_classified() {
        assert!(workspace_cargo_test("cargo test \\\n  --workspace"));
    }

    #[test]
    fn a_path_to_cargo_is_classified() {
        assert!(workspace_cargo_test(
            "/usr/local/bin/cargo test --workspace"
        ));
    }

    #[test]
    fn a_wrapped_invocation_is_classified() {
        // A wrapper program runs the cargo invocation it is given, so the
        // invocation is still a workspace cargo test. Missing it would let two
        // workspace cargo tests build the same target tree on one runner.
        assert!(workspace_cargo_test(
            "flock /cargo-target/.dil-cargo.lock \\\n  cargo test --workspace"
        ));
        assert!(workspace_cargo_test("timeout 60 cargo test --workspace"));
        assert!(workspace_cargo_test("nohup cargo test --workspace"));
    }

    #[test]
    fn a_wrapped_non_workspace_invocation_is_not_classified() {
        assert!(!workspace_cargo_test(
            "flock /cargo-target/.dil-cargo.lock \\\n  cargo llvm-cov --workspace --summary-only"
        ));
        assert!(!workspace_cargo_test(
            "flock /cargo-target/.dil-cargo.lock \\\n  cargo clippy --workspace --all-targets"
        ));
    }
}
