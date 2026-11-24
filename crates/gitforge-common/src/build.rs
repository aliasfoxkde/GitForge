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

/// The toolchain that compiled this binary, as `rustc --version` reports it,
/// or `unknown` when there was no compiler to ask.
pub const BUILD_TOOLCHAIN: &str = env!("GITFORGE_BUILD_TOOLCHAIN");

/// The moment this binary was compiled, in seconds since the Unix epoch, or
/// `unknown` when the clock could not be read.
pub const BUILD_EPOCH: &str = env!("GITFORGE_BUILD_EPOCH");

/// The value used when a build fact could not be established.
const UNKNOWN: &str = "unknown";

/// When this binary was compiled, as an RFC 3339 UTC timestamp.
///
/// The epoch is stamped at build time and formatted here, so the calendar
/// arithmetic lives in a place that is unit-tested rather than in a build
/// script that no test ever runs.
pub fn build_timestamp_utc() -> String {
    format_epoch(BUILD_EPOCH)
}

/// Format a stamped epoch as an RFC 3339 UTC timestamp.
///
/// A value that is not a moment in time degrades to `unknown` rather than
/// panicking: a binary whose clock could not be read still has to be able
/// to answer `--version`, and the release tooling refuses an `unknown`
/// build time on its own.
fn format_epoch(epoch: &str) -> String {
    let Ok(seconds) = epoch.trim().parse::<i64>() else {
        return UNKNOWN.to_string();
    };
    match chrono::DateTime::from_timestamp(seconds, 0) {
        Some(timestamp) => timestamp.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        None => UNKNOWN.to_string(),
    }
}

/// The single line a release binary prints for `--version`.
///
/// The format is part of the release contract: `gitforge-release-stamps`
/// parses `commit=`, `dirty=`, `toolchain=` and `built=` out of it to decide
/// whether a bundle may be cut, and to describe the bundle from the binaries
/// it ships, so the field names are not free-form.
///
/// The toolchain and the build time are the LAST two fields on purpose: the
/// toolchain line carries spaces (`rustc 1.99.1 (58cf8f9c3 2026-10-13)`),
/// so a parser has to anchor on the field that follows it.
pub fn version_line(component: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        "{component} {version} commit={BUILD_COMMIT} dirty={BUILD_DIRTY} toolchain={BUILD_TOOLCHAIN} built={}",
        build_timestamp_utc()
    )
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
        assert!(
            line.contains(&format!("toolchain={BUILD_TOOLCHAIN}")),
            "version line must name its toolchain: {line}"
        );
        assert!(
            line.contains(&format!("built={}", build_timestamp_utc())),
            "version line must name when it was built: {line}"
        );
    }

    #[test]
    fn the_toolchain_and_build_time_are_present_and_last() {
        let line = version_line("api");
        let toolchain = format!("toolchain={BUILD_TOOLCHAIN}");
        let built = format!("built={}", build_timestamp_utc());
        let toolchain_at = line.find(&toolchain).expect("toolchain field");
        let built_at = line.find(&built).expect("built field");
        assert!(
            toolchain_at < built_at,
            "the toolchain field must be anchored by the built field: {line}"
        );
    }

    #[test]
    fn an_epoch_is_formatted_as_utc() {
        assert_eq!(format_epoch("0"), "1970-01-01T00:00:00Z");
        assert_eq!(format_epoch("1762992000"), "2025-11-13T00:00:00Z");
    }

    #[test]
    fn an_unreadable_clock_is_reported_as_unknown() {
        assert_eq!(format_epoch("not-a-number"), UNKNOWN);
        assert_eq!(format_epoch(""), UNKNOWN);
        // Far beyond what the calendar can represent.
        assert_eq!(format_epoch("999999999999999"), UNKNOWN);
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
