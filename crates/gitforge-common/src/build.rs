//! Build provenance carried by the released binaries.
//!
//! Every release binary names the commit it was built from and whether the
//! tree it was built from was clean. The release chain asks a binary for
//! that stamp and refuses to ship one that does not match the commit the
//! bundle claims — the tool, not an operator procedure, is what makes the
//! provenance claim true.
//!
//! The stamp is produced by this crate's build script (see `build.rs`) and
//! reaches every released binary because all four services depend on
//! `gitforge-common`.

/// The commit this binary was built from, or `unknown` when the build had
/// neither a git checkout nor an explicit `GITFORGE_BUILD_COMMIT`.
pub const BUILD_COMMIT: &str = env!("GITFORGE_BUILD_COMMIT");

/// The state of the tree this binary was built from: `false` (clean),
/// `true` (modified), or `unknown` (git could not answer).
pub const BUILD_DIRTY: &str = env!("GITFORGE_BUILD_DIRTY");

/// The single line a release binary prints for `--version`.
///
/// The format is part of the release contract: `gitforge-release-stamps`
/// parses `commit=` and `dirty=` out of it to decide whether a bundle may
/// be cut, so the field names are not free-form.
pub fn version_line(component: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!("{component} {version} commit={BUILD_COMMIT} dirty={BUILD_DIRTY}")
}

/// The version line when the caller asked for the version, `None` otherwise.
///
/// Services call this before any other work so that asking a binary for its
/// provenance never starts a scheduler, opens a database, or binds a port.
pub fn version_line_if_requested(
    component: &str,
    args: impl IntoIterator<Item = String>,
) -> Option<String> {
    let requested = args
        .into_iter()
        .any(|arg| arg == "--version" || arg == "-V");
    if requested {
        Some(version_line(component))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_carries_the_release_fields() {
        let line = version_line("api");
        assert!(line.starts_with("api "), "unexpected version line: {line}");
        assert!(
            line.contains(&format!("commit={BUILD_COMMIT}")),
            "version line must name its commit: {line}"
        );
        assert!(
            line.contains(&format!("dirty={BUILD_DIRTY}")),
            "version line must name its tree state: {line}"
        );
    }

    #[test]
    fn version_flag_is_recognised_and_other_arguments_are_not() {
        assert!(version_line_if_requested("ci", vec!["--version".to_string()]).is_some());
        assert!(version_line_if_requested("ci", vec!["-V".to_string()]).is_some());
        assert!(version_line_if_requested("ci", Vec::<String>::new()).is_none());
        assert!(
            version_line_if_requested("ci", vec!["--port".to_string(), "42781".to_string()])
                .is_none()
        );
    }

    #[test]
    fn version_line_is_component_specific() {
        assert_ne!(version_line("api"), version_line("ci"));
    }
}
