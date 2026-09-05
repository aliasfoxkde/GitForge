//! Review Domain Primitives (Code Review Contract)
//!
//! Typed, serializable primitives implementing the invariants frozen in
//! `docs/architecture/adr-20260905-code-review-contract.md`:
//!
//! - [`ReviewRunState`] — run lifecycle states with monotonic terminal
//!   semantics (R3).
//! - [`PositionStatus`] — explicit finding position status (R5).
//! - [`finding_fingerprint`] — stable, content-addressed finding fingerprint
//!   (R4).
//!
//! This module deliberately contains no persistence, no I/O, and no
//! provider-specific behavior. It is the shared vocabulary that schedulers,
//! stores, and platform integrations build on.
//!
//! # Serialization
//!
//! All enums serialize to their exact ADR spelling via
//! `#[serde(rename_all = "snake_case")]` (`"pending"`, `"running"`,
//! `"succeeded"`, `"failed"`, `"cancelled"`, `"line"`, `"file"`,
//! `"deleted"`, `"unavailable"`). [`std::fmt::Display`] and [`FromStr`]
//! mirror the same strings so stringly-tailed transports round-trip
//! losslessly.
//!
//! # Fingerprint stability
//!
//! The fingerprint encoding is part of the contract. Changing the canonical
//! form changes every stored fingerprint. The canonical form is:
//!
//! 1. Each of the four fields (R4: `file`, `line`, `rule`, `message`) is
//!    UTF-8 encoded; `line = None` is encoded as the empty byte string, and
//!    `Some(n)` as the decimal ASCII digits of `n` (the line is 1-based, so
//!    the empty encoding is unambiguous).
//! 2. Each encoded field is prefixed with its byte length in decimal ASCII
//!    followed by `:` (a length-prefixed join, which cannot be confused by
//!    separator characters appearing inside field values).
//! 3. The concatenation is hashed with SHA-256 and the digest is rendered as
//!    lowercase hex, truncated to the first 16 hex characters.

use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};

/// Number of hex characters retained from the SHA-256 digest.
pub const FINGERPRINT_HEX_LEN: usize = 16;

/// Lifecycle state of a review run (ADR R3).
///
/// Progression is forward-only:
///
/// ```text
/// pending → running → succeeded | failed | cancelled
/// ```
///
/// Terminal states ([`ReviewRunState::Succeeded`],
/// [`ReviewRunState::Failed`], [`ReviewRunState::Cancelled`]) are final:
/// [`ReviewRunState::can_transition_to`] never permits a terminal run to
/// re-enter a non-terminal state, so the type-level semantics match the
/// conditional-update rules the scheduler must enforce durably.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ReviewRunState {
    /// Submitted, not yet picked up by a runner.
    Pending,
    /// A runner is executing the review against the immutable head SHA.
    Running,
    /// Terminal: the review completed and findings were produced.
    Succeeded,
    /// Terminal: the review failed permanently (e.g. provider unreachable).
    Failed,
    /// Terminal: the run was cancelled before completion.
    Cancelled,
}

impl ReviewRunState {
    /// Whether this state is terminal (ADR R3).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ReviewRunState::Succeeded | ReviewRunState::Failed | ReviewRunState::Cancelled
        )
    }

    /// Whether the transition `self → next` is permitted.
    ///
    /// Rules:
    /// - a terminal state permits only remaining in itself (R3: terminal
    ///   states are final);
    /// - a non-terminal state may stay in place (idempotent re-assert) or
    ///   move forward in the lifecycle, never backward.
    pub fn can_transition_to(self, next: ReviewRunState) -> bool {
        if self.is_terminal() {
            self == next
        } else {
            next >= self
        }
    }
}

impl fmt::Display for ReviewRunState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ReviewRunState::Pending => "pending",
            ReviewRunState::Running => "running",
            ReviewRunState::Succeeded => "succeeded",
            ReviewRunState::Failed => "failed",
            ReviewRunState::Cancelled => "cancelled",
        };
        f.write_str(s)
    }
}

impl FromStr for ReviewRunState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(ReviewRunState::Pending),
            "running" => Ok(ReviewRunState::Running),
            "succeeded" => Ok(ReviewRunState::Succeeded),
            "failed" => Ok(ReviewRunState::Failed),
            "cancelled" => Ok(ReviewRunState::Cancelled),
            other => Err(format!("unknown review run state: {other:?}")),
        }
    }
}

/// Explicit position status of a review finding (ADR R5).
///
/// `line = null` on a finding is only valid when the status is
/// [`PositionStatus::File`], [`PositionStatus::Deleted`], or
/// [`PositionStatus::Unavailable`]; a [`PositionStatus::Line`] finding must
/// carry both `file` and a 1-based `line`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PositionStatus {
    /// Finding applies to a specific line (both `file` and `line` are set).
    Line,
    /// Finding is file-level; `line` is `null`.
    File,
    /// The pointed-to line was deleted in the current diff; retained with
    /// `line = null`.
    Deleted,
    /// The position cannot be resolved (e.g. file renamed); must never be
    /// posted as an inline comment.
    Unavailable,
}

impl PositionStatus {
    /// Whether this status permits `line = null` on a finding (ADR R5:
    /// every status except [`PositionStatus::Line`]).
    pub fn allows_null_line(self) -> bool {
        !matches!(self, PositionStatus::Line)
    }

    /// Whether the finding is anchored to a concrete resolvable line and is
    /// therefore eligible to be posted inline. [`PositionStatus::Unavailable`]
    /// findings are excluded by contract (ADR R5).
    pub fn is_line_position(self) -> bool {
        matches!(self, PositionStatus::Line)
    }
}

impl fmt::Display for PositionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            PositionStatus::Line => "line",
            PositionStatus::File => "file",
            PositionStatus::Deleted => "deleted",
            PositionStatus::Unavailable => "unavailable",
        };
        f.write_str(s)
    }
}

impl FromStr for PositionStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "line" => Ok(PositionStatus::Line),
            "file" => Ok(PositionStatus::File),
            "deleted" => Ok(PositionStatus::Deleted),
            "unavailable" => Ok(PositionStatus::Unavailable),
            other => Err(format!("unknown position status: {other:?}")),
        }
    }
}

/// Compute the stable fingerprint of a finding (ADR R4).
///
/// Inputs are exactly the four contract fields:
///
/// - `file` — path relative to the repository root (empty for findings with
///   no resolvable file, e.g. the R6 sentinel);
/// - `line` — the 1-based line number, or `None` for file-level findings;
/// - `rule` — identifier of the rule that triggered the finding;
/// - `message` — the exact content string emitted by the model.
///
/// The result is deterministic: identical fields always produce the same
/// 16-hex-character fingerprint, enabling deduplication across retried runs
/// on the same head SHA. See the module docs for the canonical encoding.
pub fn finding_fingerprint(file: &str, line: Option<u32>, rule: &str, message: &str) -> String {
    let line_str = line.map(|l| l.to_string()).unwrap_or_default();
    let mut hasher = Sha256::new();
    for field in [file, line_str.as_str(), rule, message] {
        let bytes = field.as_bytes();
        // Length-prefix each field so separator characters inside a value
        // cannot produce the same byte stream as a different field split.
        write_len_prefixed(&mut hasher, bytes);
    }
    let digest = hasher.finalize();
    hex::encode(&digest[..FINGERPRINT_HEX_LEN / 2])
}

fn write_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update(bytes.len().to_string().as_bytes());
    hasher.update(b":");
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_RUN_STATES: [ReviewRunState; 5] = [
        ReviewRunState::Pending,
        ReviewRunState::Running,
        ReviewRunState::Succeeded,
        ReviewRunState::Failed,
        ReviewRunState::Cancelled,
    ];

    const ALL_POSITION_STATUSES: [PositionStatus; 4] = [
        PositionStatus::Line,
        PositionStatus::File,
        PositionStatus::Deleted,
        PositionStatus::Unavailable,
    ];

    #[test]
    fn run_state_serializes_with_adr_spelling() {
        let expected = [
            (ReviewRunState::Pending, "\"pending\""),
            (ReviewRunState::Running, "\"running\""),
            (ReviewRunState::Succeeded, "\"succeeded\""),
            (ReviewRunState::Failed, "\"failed\""),
            (ReviewRunState::Cancelled, "\"cancelled\""),
        ];
        for (state, json) in expected {
            assert_eq!(serde_json::to_string(&state).unwrap(), json);
            assert_eq!(serde_json::from_str::<ReviewRunState>(json).unwrap(), state);
        }
    }

    #[test]
    fn run_state_display_and_from_str_round_trip() {
        for state in ALL_RUN_STATES {
            let rendered = state.to_string();
            assert_eq!(rendered.parse::<ReviewRunState>().unwrap(), state);
        }
        assert!("queued".parse::<ReviewRunState>().is_err());
        assert!("".parse::<ReviewRunState>().is_err());
    }

    #[test]
    fn terminal_states_are_recognized() {
        assert!(!ReviewRunState::Pending.is_terminal());
        assert!(!ReviewRunState::Running.is_terminal());
        for state in [
            ReviewRunState::Succeeded,
            ReviewRunState::Failed,
            ReviewRunState::Cancelled,
        ] {
            assert!(state.is_terminal());
        }
    }

    #[test]
    fn transition_matrix_matches_monotonic_contract() {
        // Exhaustive truth table (ADR R3): terminals are final, non-terminals
        // never move backward.
        let allowed = [
            (ReviewRunState::Pending, ReviewRunState::Pending),
            (ReviewRunState::Pending, ReviewRunState::Running),
            (ReviewRunState::Pending, ReviewRunState::Succeeded),
            (ReviewRunState::Pending, ReviewRunState::Failed),
            (ReviewRunState::Pending, ReviewRunState::Cancelled),
            (ReviewRunState::Running, ReviewRunState::Running),
            (ReviewRunState::Running, ReviewRunState::Succeeded),
            (ReviewRunState::Running, ReviewRunState::Failed),
            (ReviewRunState::Running, ReviewRunState::Cancelled),
            (ReviewRunState::Succeeded, ReviewRunState::Succeeded),
            (ReviewRunState::Failed, ReviewRunState::Failed),
            (ReviewRunState::Cancelled, ReviewRunState::Cancelled),
        ];
        for from in ALL_RUN_STATES {
            for to in ALL_RUN_STATES {
                let expected = allowed.contains(&(from, to));
                assert!(
                    from.can_transition_to(to) == expected,
                    "unexpected transition {from} → {to}"
                );
            }
        }
    }

    #[test]
    fn terminal_run_can_never_reenter_non_terminal_state() {
        for terminal in [
            ReviewRunState::Succeeded,
            ReviewRunState::Failed,
            ReviewRunState::Cancelled,
        ] {
            for escape in [ReviewRunState::Pending, ReviewRunState::Running] {
                assert!(
                    !terminal.can_transition_to(escape),
                    "terminal state {terminal} must not transition to {escape}"
                );
            }
        }
    }

    #[test]
    fn position_status_serializes_with_adr_spelling() {
        let expected = [
            (PositionStatus::Line, "\"line\""),
            (PositionStatus::File, "\"file\""),
            (PositionStatus::Deleted, "\"deleted\""),
            (PositionStatus::Unavailable, "\"unavailable\""),
        ];
        for (status, json) in expected {
            assert_eq!(serde_json::to_string(&status).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<PositionStatus>(json).unwrap(),
                status
            );
        }
        assert!("inline".parse::<PositionStatus>().is_err());
    }

    #[test]
    fn position_status_null_line_and_inline_rules_match_adr() {
        // ADR R5 table: only `line` requires a line number; every other
        // status carries `line = null`.
        for status in ALL_POSITION_STATUSES {
            assert_eq!(status.allows_null_line(), status != PositionStatus::Line);
            assert_eq!(status.is_line_position(), status == PositionStatus::Line);
        }
    }

    #[test]
    fn fingerprint_matches_reference_sha256_vectors() {
        // Golden vectors computed independently (SHA-256 over the canonical
        // length-prefixed form documented in the module docs).
        assert_eq!(
            finding_fingerprint(
                "src/main.rs",
                Some(42),
                "no-hardcoded-secret",
                "hardcoded password detected"
            ),
            "75683dc2621332f3"
        );
        assert_eq!(
            finding_fingerprint(
                "configs/settings.yaml",
                None,
                "file-too-large",
                "file exceeds size limit"
            ),
            "9f54f5053c9218e5"
        );
        // R6-style sentinel: no file, no line, internal rule, empty message.
        assert_eq!(
            finding_fingerprint("", None, "model-output-invalid", ""),
            "301056386d24e9ec"
        );
    }

    #[test]
    fn fingerprint_is_truncated_sha256_with_documented_prefix() {
        // Independently computed full SHA-256 digest of the canonical byte
        // stream `11:src/main.rs2:4219:no-hardcoded-secret27:hardcoded
        // password detected`; the fingerprint must be exactly its first 16
        // hex characters.
        let full_digest = "75683dc2621332f398f765588532a60e86ecbbf3c4863db07949075482b190f3";
        let fp = finding_fingerprint(
            "src/main.rs",
            Some(42),
            "no-hardcoded-secret",
            "hardcoded password detected",
        );
        assert_eq!(fp.len(), FINGERPRINT_HEX_LEN);
        assert_eq!(fp, &full_digest[..FINGERPRINT_HEX_LEN]);
    }

    #[test]
    fn fingerprint_is_deterministic_and_handles_unicode() {
        let first = finding_fingerprint("src/ünicode/naïve.rs", Some(7), "rule-é", "café ☕");
        let second = finding_fingerprint("src/ünicode/naïve.rs", Some(7), "rule-é", "café ☕");
        assert_eq!(first, second);
        assert_eq!(first, "247f54e44ea50f52");
    }

    #[test]
    fn fingerprint_length_prefix_prevents_ambiguity() {
        // Without length prefixes, field boundaries ("a/b" + "" vs "a" + "b")
        // could collapse to the same byte stream.
        let a = finding_fingerprint("a/b", None, "", "");
        let b = finding_fingerprint("a", None, "b", "");
        assert_ne!(a, b);
        assert_eq!(a, "e4c591dcd671bcfa");
        assert_eq!(b, "0a16e906004840e2");
    }

    #[test]
    fn fingerprint_is_sensitive_to_each_field() {
        let base = finding_fingerprint("src/lib.rs", Some(3), "rule", "message");
        assert_ne!(
            base,
            finding_fingerprint("src/other.rs", Some(3), "rule", "message")
        );
        assert_ne!(
            base,
            finding_fingerprint("src/lib.rs", Some(4), "rule", "message")
        );
        assert_ne!(
            base,
            finding_fingerprint("src/lib.rs", None, "rule", "message")
        );
        assert_ne!(
            base,
            finding_fingerprint("src/lib.rs", Some(3), "other-rule", "message")
        );
        assert_ne!(
            base,
            finding_fingerprint("src/lib.rs", Some(3), "rule", "other message")
        );
    }
}
