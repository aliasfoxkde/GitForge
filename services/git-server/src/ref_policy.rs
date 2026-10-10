//! Ref-update policy enforcement for pushes (#240).
//!
//! GitForge spawns a real `git-receive-pack` on both transports, but the
//! update command list is fully known before any pack data matters: Smart
//! HTTP buffers the whole request body, and the SSH transport receives the
//! command list as a self-delimiting pkt-line block terminated by a flush
//! packet. That lets the repository's ref-update policy be evaluated BEFORE
//! spawning the child and declined with a standard receive-pack status
//! report (`ng <ref> <reason>`), which git clients render as
//! `! [remote rejected] <ref> (reason)`.
//!
//! Two mechanisms cooperate:
//!
//! - **Required status checks** are evaluated here against
//!   `pipeline_runs` (a commit advances a branch only when the pipelines
//!   named by `repositories.required_checks` have succeeded on it).
//! - **Non-fast-forward denial** is delegated to git's own
//!   `receive.denyNonFastForwards` repository config, which receive-pack
//!   enforces with quarantine-correct ancestry checks and per-ref status —
//!   see `gitforge-core::storage` for where the config is written.

use gitforge_common::{is_zero_hash, RepoId};
use gitforge_db::queries::{
    evaluate_required_checks, required_checks_satisfied, CommitRunStatus, PipelineRunQueries,
    RepoQueries,
};
use gitforge_db::Pool;

/// Upper bound on the buffered receive-pack command list. A legitimate
/// command list is a few hundred bytes per ref; anything near this cap is a
/// hostile stream and the push is declined rather than buffered forever.
pub const MAX_COMMAND_LIST_BYTES: usize = 1024 * 1024;

/// One ref update parsed from a receive-pack command list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveUpdate {
    pub old_hash: String,
    pub new_hash: String,
    pub ref_name: String,
}

/// Parse receive-pack command lines out of a raw pkt-line stream.
///
/// Stops at the terminating flush packet (or the first malformed frame);
/// callers on the SSH path use [`command_list_len`] to make sure the list
/// is complete before calling this.
pub fn parse_receive_updates(input: &[u8]) -> Vec<ReceiveUpdate> {
    let mut updates = Vec::new();
    let mut offset = 0;
    while offset + 4 <= input.len() {
        let Ok(length) =
            usize::from_str_radix(&String::from_utf8_lossy(&input[offset..offset + 4]), 16)
        else {
            break;
        };
        if length == 0 {
            break;
        }
        if length < 4 || offset + length > input.len() {
            break;
        }
        let payload = &input[offset + 4..offset + length];
        if let Ok(line) = std::str::from_utf8(payload) {
            let fields: Vec<&str> = line
                .split('\0')
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .collect();
            if fields.len() >= 3 {
                updates.push(ReceiveUpdate {
                    old_hash: fields[0].to_string(),
                    new_hash: fields[1].to_string(),
                    ref_name: fields[2].to_string(),
                });
            }
        }
        offset += length;
    }
    updates
}

/// Byte length of the receive-pack command list at the start of `input`,
/// including its terminating flush packet.
///
/// Returns `None` while the stream has not yet delivered the complete list,
/// and `Some(length)` otherwise. The command list is always the first thing
/// a push client sends, so buffering exactly this prefix is enough to
/// evaluate policy before anything reaches `git-receive-pack`.
pub fn command_list_len(input: &[u8]) -> Option<usize> {
    let mut offset = 0;
    loop {
        if offset + 4 > input.len() {
            return None;
        }
        let length =
            usize::from_str_radix(&String::from_utf8_lossy(&input[offset..offset + 4]), 16).ok()?;
        if length == 0 {
            return Some(offset + 4);
        }
        if length < 4 || offset + length > input.len() {
            return None;
        }
        offset += length;
    }
}

/// Policy verdict for one push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefPolicyDecision {
    /// Forward the push to `git-receive-pack` unchanged.
    Allow,
    /// Decline the push; one reason per declined ref.
    Reject(Vec<(String, String)>),
}

impl RefPolicyDecision {
    /// Test-only convenience; production callers match on the variant so
    /// every Reject reason is handled explicitly.
    #[cfg(test)]
    pub(crate) fn is_allow(&self) -> bool {
        matches!(self, RefPolicyDecision::Allow)
    }
}

/// Evaluate the repository's ref-update policy against a parsed command list.
///
/// Only `refs/heads/*` updates that install a commit are gated: tag pushes,
/// other ref namespaces, and branch deletions keep today's behavior, and a
/// repository without required checks never blocks here (non-fast-forward
/// denial, the other policy knob, lives in the repository's git config).
///
/// The check fails closed: when the policy cannot be read, every branch
/// update is declined rather than waved through.
pub async fn evaluate_ref_policy(
    pool: &Pool,
    repo_id: RepoId,
    updates: &[ReceiveUpdate],
) -> RefPolicyDecision {
    let gated: Vec<&ReceiveUpdate> = updates
        .iter()
        .filter(|update| {
            update.ref_name.starts_with("refs/heads/") && !is_zero_hash(&update.new_hash)
        })
        .collect();
    if gated.is_empty() {
        return RefPolicyDecision::Allow;
    }

    let repo = match RepoQueries::get(pool, repo_id).await {
        Ok(Some(repo)) => repo,
        Ok(None) => {
            return RefPolicyDecision::Reject(
                gated
                    .iter()
                    .map(|update| {
                        (
                            update.ref_name.clone(),
                            "ref-update policy unavailable: repository not found".to_string(),
                        )
                    })
                    .collect(),
            );
        }
        Err(error) => {
            tracing::error!(
                repo_id = %repo_id,
                error = %error,
                "ref-update policy lookup failed; declining branch updates"
            );
            return RefPolicyDecision::Reject(
                gated
                    .iter()
                    .map(|update| {
                        (
                            update.ref_name.clone(),
                            "ref-update policy unavailable".to_string(),
                        )
                    })
                    .collect(),
            );
        }
    };

    if !repo.has_required_checks() {
        return RefPolicyDecision::Allow;
    }

    // A push commonly installs the same commit on several branches; evaluate
    // each distinct commit once.
    let mut reasons: Vec<(String, String)> = Vec::new();
    let mut cache: Vec<(String, Vec<CommitRunStatus>)> = Vec::new();
    for update in gated {
        let commit = update.new_hash.to_lowercase();
        let statuses = match cache.iter().find(|(seen, _)| *seen == commit) {
            Some((_, statuses)) => statuses.clone(),
            None => {
                let statuses = PipelineRunQueries::list_commit_statuses(pool, repo_id, &commit)
                    .await
                    .unwrap_or_else(|error| {
                        tracing::error!(
                            repo_id = %repo_id,
                            commit = %commit,
                            error = %error,
                            "commit status lookup failed; treating commit as not green"
                        );
                        Vec::new()
                    });
                cache.push((commit, statuses.clone()));
                statuses
            }
        };
        let checks = evaluate_required_checks(&repo.required_checks, &statuses);
        if required_checks_satisfied(&checks) {
            continue;
        }
        let unmet = checks
            .iter()
            .filter(|check| !check.is_green())
            .map(|check| match check.status.as_deref() {
                Some(status) => format!("{} is {}", check.check, status),
                None => format!("{} has not run", check.check),
            })
            .collect::<Vec<_>>()
            .join(", ");
        tracing::info!(
            repo_id = %repo_id,
            ref_name = %update.ref_name,
            commit = %update.new_hash,
            "branch update declined by ref-update policy"
        );
        reasons.push((
            update.ref_name.clone(),
            format!("required checks not green: {unmet}"),
        ));
    }

    if reasons.is_empty() {
        RefPolicyDecision::Allow
    } else {
        RefPolicyDecision::Reject(reasons)
    }
}

/// Frame one pkt-line payload.
fn pkt_line(payload: &str) -> Vec<u8> {
    format!("{:04x}{}", payload.len() + 4, payload).into_bytes()
}

/// Build the receive-pack status report declining `reasons`.
///
/// The report claims a clean unpack because nothing was unpacked and every
/// refusal is carried per ref — exactly what a client sees when a real
/// pre-receive hook declines every update.
pub fn synthesize_rejection_report(reasons: &[(String, String)]) -> Vec<u8> {
    let mut report = pkt_line("unpack ok\n");
    for (ref_name, reason) in reasons {
        report.extend(pkt_line(&format!("ng {ref_name} {reason}\n")));
    }
    report.extend_from_slice(b"0000");
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(old: &str, new: &str, ref_name: &str) -> ReceiveUpdate {
        ReceiveUpdate {
            old_hash: old.to_string(),
            new_hash: new.to_string(),
            ref_name: ref_name.to_string(),
        }
    }

    const ZERO: &str = "0000000000000000000000000000000000000000";
    const COMMIT_A: &str = "1111111111111111111111111111111111111111";
    const COMMIT_B: &str = "2222222222222222222222222222222222222222";

    fn command_frame(old: &str, new: &str, ref_name: &str) -> Vec<u8> {
        let payload = format!("{old} {new} {ref_name}\0report-status\n");
        let mut frame = format!("{:04x}", payload.len() + 4).into_bytes();
        frame.extend_from_slice(payload.as_bytes());
        frame
    }

    #[test]
    fn test_parse_receive_updates_reads_command_list() {
        let mut input = command_frame(ZERO, COMMIT_A, "refs/heads/main");
        input.extend_from_slice(b"0000");
        assert_eq!(
            parse_receive_updates(&input),
            vec![update(ZERO, COMMIT_A, "refs/heads/main")]
        );
    }

    #[test]
    fn test_parse_receive_updates_stops_at_flush_and_garbage() {
        let mut input = command_frame(ZERO, COMMIT_A, "refs/heads/main");
        input.extend_from_slice(b"0000");
        // Pack data after the flush must not be read as more commands.
        input.extend_from_slice(b"PACK\x00\x00\x00\x02garbage");
        assert_eq!(
            parse_receive_updates(&input),
            vec![update(ZERO, COMMIT_A, "refs/heads/main")]
        );
        assert_eq!(parse_receive_updates(b"zzzz"), Vec::new());
        assert_eq!(parse_receive_updates(b""), Vec::new());
    }

    #[test]
    fn test_command_list_len_measures_through_flush() {
        let mut input = command_frame(ZERO, COMMIT_A, "refs/heads/main");
        input.extend_from_slice(&command_frame(COMMIT_A, COMMIT_B, "refs/heads/next"));
        let complete_len = input.len() + 4;
        input.extend_from_slice(b"0000");

        assert_eq!(command_list_len(&input), Some(complete_len));
        assert_eq!(command_list_len(&input[..8]), None);
        assert_eq!(command_list_len(&input[..complete_len - 1]), None);

        let mut split = input[..complete_len].to_vec();
        split.extend_from_slice(b"PACK");
        assert_eq!(command_list_len(&split), Some(complete_len));
    }

    #[test]
    fn test_rejection_report_is_valid_pkt_line_stream() {
        let report = synthesize_rejection_report(&[(
            "refs/heads/main".to_string(),
            "required checks not green: ci is failed".to_string(),
        )]);
        let expected =
            "000eunpack ok\n003fng refs/heads/main required checks not green: ci is failed\n0000";
        assert_eq!(String::from_utf8(report).unwrap(), expected);
    }

    #[tokio::test]
    async fn test_policy_without_required_checks_allows_branch_updates() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let user = gitforge_db::models::User::new(
            "pusher".to_string(),
            "pusher@example.com".to_string(),
            "hash".to_string(),
        );
        gitforge_db::queries::UserQueries::create(&pool, &user)
            .await
            .unwrap();
        let repo = gitforge_db::models::Repository::new(
            "policy-free".to_string(),
            user.id,
            "/git/policy-free".to_string(),
        );
        gitforge_db::queries::RepoQueries::create(&pool, &repo)
            .await
            .unwrap();

        let updates = vec![update(ZERO, COMMIT_A, "refs/heads/main")];
        assert!(evaluate_ref_policy(&pool, repo.id, &updates)
            .await
            .is_allow());
    }

    #[tokio::test]
    async fn test_required_checks_gate_branch_updates_only() {
        let pool = Pool::memory().await.unwrap();
        pool.migrate().await.unwrap();
        let user = gitforge_db::models::User::new(
            "gated-pusher".to_string(),
            "gated-pusher@example.com".to_string(),
            "hash".to_string(),
        );
        gitforge_db::queries::UserQueries::create(&pool, &user)
            .await
            .unwrap();
        let repo = gitforge_db::models::Repository::new(
            "gated".to_string(),
            user.id,
            "/git/gated".to_string(),
        );
        gitforge_db::queries::RepoQueries::create(&pool, &repo)
            .await
            .unwrap();
        gitforge_db::queries::RepoQueries::update_policy(&pool, repo.id, &["ci".to_string()], true)
            .await
            .unwrap();

        // Deletions, tags, and empty command lists are never gated.
        let ungated = vec![
            update(COMMIT_A, ZERO, "refs/heads/main"),
            update(ZERO, COMMIT_B, "refs/tags/v1"),
        ];
        assert!(evaluate_ref_policy(&pool, repo.id, &ungated)
            .await
            .is_allow());
        assert!(evaluate_ref_policy(&pool, repo.id, &[]).await.is_allow());

        // A branch advance to a commit with no green ci run is declined…
        let decision = evaluate_ref_policy(
            &pool,
            repo.id,
            &[update(COMMIT_A, COMMIT_B, "refs/heads/main")],
        )
        .await;
        let RefPolicyDecision::Reject(reasons) = decision else {
            panic!("expected rejection for not-green commit");
        };
        assert_eq!(reasons.len(), 1);
        assert_eq!(reasons[0].0, "refs/heads/main");
        assert!(reasons[0].1.contains("ci has not run"));

        // …including when the only run failed.
        let pipeline = gitforge_db::models::Pipeline {
            id: gitforge_common::PipelineId::new(),
            repo_id: repo.id,
            name: "ci".to_string(),
            trigger_type: "push".to_string(),
            config: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };
        gitforge_db::queries::PipelineQueries::create(&pool, &pipeline)
            .await
            .unwrap();
        let run = gitforge_db::models::PipelineRun::new(
            pipeline.id,
            repo.id,
            "push".to_string(),
            COMMIT_A.to_string(),
        );
        gitforge_db::queries::PipelineRunQueries::create(&pool, &run)
            .await
            .unwrap();
        gitforge_db::queries::PipelineRunQueries::update_status(&pool, run.id, "failed")
            .await
            .unwrap();
        let decision = evaluate_ref_policy(
            &pool,
            repo.id,
            &[update(COMMIT_B, COMMIT_A, "refs/heads/main")],
        )
        .await;
        let RefPolicyDecision::Reject(reasons) = decision else {
            panic!("expected rejection for failed required check");
        };
        assert!(reasons[0].1.contains("ci is failed"));

        // And a retrigger that succeeds — a NEW run, since update_status
        // refuses to rewrite a terminal verdict (F24) — lets the same
        // commit advance: the latest run per pipeline wins.
        let retried = gitforge_db::models::PipelineRun::new(
            pipeline.id,
            repo.id,
            "push".to_string(),
            COMMIT_A.to_string(),
        );
        gitforge_db::queries::PipelineRunQueries::create(&pool, &retried)
            .await
            .unwrap();
        gitforge_db::queries::PipelineRunQueries::update_status(&pool, retried.id, "succeeded")
            .await
            .unwrap();
        assert!(evaluate_ref_policy(
            &pool,
            repo.id,
            &[update(COMMIT_B, COMMIT_A, "refs/heads/main")]
        )
        .await
        .is_allow());
    }

    #[test]
    fn test_max_command_list_bytes_bounds_ssh_buffering() {
        // A one-ref command list is far below the cap; the constant exists so
        // the SSH buffer logic has a single named bound to enforce.
        let mut input = command_frame(ZERO, COMMIT_A, "refs/heads/main");
        input.extend_from_slice(b"0000");
        assert!(input.len() < MAX_COMMAND_LIST_BYTES);
    }
}
