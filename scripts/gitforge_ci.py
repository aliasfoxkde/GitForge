#!/usr/bin/env python3
"""Publish a GitHub candidate to GitForge and await its durable run receipt."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


SHA_PATTERN = re.compile(r"^(?:[0-9a-f]{40}|[0-9a-f]{64})$")
ID_PATTERN = re.compile(r"^[0-9]+$")
TERMINAL_FAILURES = {"cancelled", "canceled", "error", "failed", "failure"}


def candidate_ref(
    event_name: str,
    pull_request_number: str | None,
    run_id: str,
    run_attempt: str,
) -> str:
    """Return a unique, PR-scoped ref that cannot update a protected branch."""
    if not ID_PATTERN.fullmatch(run_id) or not ID_PATTERN.fullmatch(run_attempt):
        raise ValueError("GitHub run ID and attempt must be numeric")

    run_key = f"{run_id}-{run_attempt}"
    if event_name == "pull_request":
        if pull_request_number is None or not ID_PATTERN.fullmatch(pull_request_number):
            raise ValueError("pull_request events require a numeric PR number")
        return f"refs/heads/github-ci/pull/{pull_request_number}/run/{run_key}"
    if event_name == "push":
        return f"refs/heads/github-ci/push/run/{run_key}"
    raise ValueError(f"unsupported GitHub event for GitForge CI: {event_name}")


def _git(args: list[str], cwd: str | Path, env: dict[str, str]) -> str:
    result = subprocess.run(
        ["git", *args],
        cwd=cwd,
        env=env,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip() or "git command failed"
        raise RuntimeError(f"git {' '.join(args[:2])} failed: {detail}")
    return result.stdout.strip()


def publish_candidate(
    remote_url: str,
    ref_name: str,
    expected_sha: str,
    *,
    cwd: str | Path = ".",
    username: str | None = None,
    token: str | None = None,
) -> None:
    """Push the checked-out commit to a fresh candidate ref and verify its OID."""
    if not remote_url:
        raise ValueError("GitForge Git remote URL is required")
    if not SHA_PATTERN.fullmatch(expected_sha):
        raise ValueError("candidate SHA must be a full Git object ID")
    if (username is None) != (token is None):
        raise ValueError("GitForge Git username and token must be provided together")

    env = os.environ.copy()
    env["GIT_TERMINAL_PROMPT"] = "0"
    actual_sha = _git(["rev-parse", "--verify", "HEAD^{commit}"], cwd, env)
    if actual_sha != expected_sha:
        raise ValueError("checked-out commit does not match the expected GitHub SHA")
    _git(["check-ref-format", ref_name], cwd, env)

    with tempfile.TemporaryDirectory(prefix="gitforge-askpass-") as temporary:
        askpass_path = Path(temporary) / "askpass"
        if username is not None and token is not None:
            askpass_path.write_text(
                "#!/bin/sh\n"
                "case \"$1\" in\n"
                "  *sername*) printf '%s\\n' \"$GITFORGE_GIT_USERNAME\" ;;\n"
                "  *assword*) printf '%s\\n' \"$GITFORGE_GIT_TOKEN\" ;;\n"
                "  *) exit 1 ;;\n"
                "esac\n",
                encoding="utf-8",
            )
            askpass_path.chmod(0o700)
            env.update(
                {
                    "GIT_ASKPASS": str(askpass_path),
                    "GITFORGE_GIT_USERNAME": username,
                    "GITFORGE_GIT_TOKEN": token,
                }
            )

        prior = _git(["ls-remote", "--refs", remote_url, ref_name], cwd, env)
        if prior:
            remote_sha = prior.split(maxsplit=1)[0]
            if remote_sha == expected_sha:
                return
            raise RuntimeError("candidate ref already exists with a different commit")

        _git(["push", remote_url, f"HEAD:{ref_name}"], cwd, env)
        verified = _git(["ls-remote", "--refs", remote_url, ref_name], cwd, env)
        if not verified or verified.split(maxsplit=1)[0] != expected_sha:
            raise RuntimeError("GitForge candidate ref did not resolve to the expected SHA")


def _request_json(
    method: str,
    url: str,
    token: str,
    payload: dict[str, Any] | None = None,
) -> tuple[int, dict[str, Any]]:
    body = json.dumps(payload).encode("utf-8") if payload is not None else None
    request = Request(
        url,
        data=body,
        method=method,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/json",
            "Content-Type": "application/json",
        },
    )
    try:
        with urlopen(request, timeout=30) as response:
            status = response.status
            response_body = response.read()
    except HTTPError as error:
        status = error.code
        response_body = error.read()
    except URLError as error:
        raise RuntimeError(f"GitForge request failed: {error.reason}") from error

    try:
        parsed = json.loads(response_body or b"{}")
    except json.JSONDecodeError as error:
        raise RuntimeError(f"GitForge returned invalid JSON (HTTP {status})") from error
    if not isinstance(parsed, dict):
        raise RuntimeError(f"GitForge returned a non-object response (HTTP {status})")
    return status, parsed


def enqueue_and_wait(
    scheduler_url: str,
    repo_id: str,
    ref_name: str,
    sha: str,
    token: str,
    *,
    old_sha: str = "",
    timeout_seconds: int = 1800,
    interval_seconds: int = 15,
) -> str:
    """Accept immediate or asynchronous enqueue receipts and return the run ID."""
    if not token:
        raise ValueError("GitForge API token is required")
    if not SHA_PATTERN.fullmatch(sha):
        raise ValueError("candidate SHA must be a full Git object ID")
    if old_sha and not SHA_PATTERN.fullmatch(old_sha):
        raise ValueError("old SHA must be empty or a full Git object ID")
    if timeout_seconds <= 0 or interval_seconds <= 0:
        raise ValueError("poll timeout and interval must be positive")

    base_url = scheduler_url.rstrip("/")
    status, receipt = _request_json(
        "POST",
        f"{base_url}/pipelines/trigger",
        token,
        {
            "repo_id": repo_id,
            "ref_name": ref_name,
            "old_hash": old_sha,
            "new_hash": sha,
            "working_dir": None,
        },
    )
    if status not in {200, 201, 202}:
        raise RuntimeError(f"GitForge enqueue rejected the request (HTTP {status})")

    run_id = receipt.get("pipeline_run_id")
    if isinstance(run_id, str) and run_id:
        return run_id

    trigger_id = receipt.get("trigger_id")
    if not isinstance(trigger_id, str) or not trigger_id:
        raise RuntimeError("GitForge accepted the request without a run ID or trigger ID")

    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        status, state = _request_json(
            "GET",
            f"{base_url}/pipeline-trigger-requests/{trigger_id}",
            token,
        )
        if status != 200:
            raise RuntimeError(f"GitForge trigger status request failed (HTTP {status})")
        run_id = state.get("pipeline_run_id")
        if isinstance(run_id, str) and run_id:
            return run_id

        request_status = str(state.get("status", "")).lower()
        if request_status in TERMINAL_FAILURES or request_status == "completed":
            raise RuntimeError(
                f"GitForge trigger reached terminal status without a run ID: {request_status}"
            )
        time.sleep(min(interval_seconds, max(0, deadline - time.monotonic())))

    raise TimeoutError("GitForge trigger was accepted but no pipeline run ID was assigned in time")


def _candidate_ref_command() -> int:
    ref = candidate_ref(
        os.environ.get("GITHUB_EVENT_NAME", ""),
        os.environ.get("GITFORGE_PR_NUMBER") or None,
        os.environ.get("GITHUB_RUN_ID", ""),
        os.environ.get("GITHUB_RUN_ATTEMPT", ""),
    )
    print(ref)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("candidate-ref", help="print a unique safe candidate ref")

    publish = commands.add_parser("publish", help="push and verify the checked-out candidate")
    publish.add_argument("--remote", required=True)
    publish.add_argument("--ref", required=True)
    publish.add_argument("--sha", required=True)
    publish.add_argument("--cwd", default=".")

    trigger = commands.add_parser("trigger", help="enqueue and await a GitForge pipeline run")
    trigger.add_argument("--scheduler-url", required=True)
    trigger.add_argument("--repo-id", required=True)
    trigger.add_argument("--ref-name", required=True)
    trigger.add_argument("--sha", required=True)
    trigger.add_argument("--old-sha", default=os.environ.get("GITFORGE_OLD_SHA", ""))
    trigger.add_argument("--timeout", type=int, default=1800)
    trigger.add_argument("--interval", type=int, default=15)

    args = parser.parse_args(argv)
    try:
        if args.command == "candidate-ref":
            return _candidate_ref_command()
        if args.command == "publish":
            publish_candidate(
                args.remote,
                args.ref,
                args.sha,
                cwd=args.cwd,
                username=os.environ.get("GITFORGE_GIT_USERNAME"),
                token=os.environ.get("GITFORGE_GIT_TOKEN"),
            )
            print(args.ref)
            return 0
        run_id = enqueue_and_wait(
            args.scheduler_url,
            args.repo_id,
            args.ref_name,
            args.sha,
            os.environ.get("GITFORGE_API_TOKEN", ""),
            old_sha=args.old_sha,
            timeout_seconds=args.timeout,
            interval_seconds=args.interval,
        )
        print(run_id)
        return 0
    except (OSError, RuntimeError, TimeoutError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
