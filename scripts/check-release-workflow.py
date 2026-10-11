#!/usr/bin/env python3
"""Release workflow contract checker.

Pins .github/workflows/release.yml to its publication contract. The
release workflow builds real binaries and (on tag pushes) publishes a
real GitHub Release, so a silent regression there is expensive in both
directions: a toolchain regression burns a cross-platform build, and a
gating regression publishes. This checker makes both fail closed at
push time instead.

The contract (the same gate runs in-file as the `contract` job, in
.gitforge.yml's fmt job, and in rust-ci.yml's workflow-lint job):

  1. Triggers: tag push `v*` (release semantics) + workflow_dispatch
     (build/validation runs).
  2. Publication: the `release` job's `if` requires repository owner,
     push event, tag ref type, AND the `refs/tags/v` prefix — a
     workflow_dispatch on a branch can build but can never publish.
  3. Toolchain: every build platform installs the repository's pinned
     dtolnay/rust-toolchain revision — the same pin as rust-ci.yml —
     with `toolchain` and the platform `targets` declared, and no
     cargo/rustc/rustup command runs before that step. The self-hosted
     Fedora job must also expose the runner's rustup on PATH first (the
     PR #293 defect class) and run every cargo build/test step with
     TMPDIR=${{ runner.temp }} (the quota-constrained /tmp defect).
  4. A `cargo --version` check precedes the first build/test step.
  5. The `contract` job gates every build job, and the release job
     depends on the contract gate and every build.
  6. The release payload is preserved: SBOM, attest, checksums,
     release action, and the elevated publish permissions.
  7. No job-level env entry uses the runner context (the PR #293 §8
     workflow-file rejection class).

Parse-only, stdlib-only (the dsc-ci-rust image ships python3 without
PyYAML), no network, deterministic; exits 1 on any violation or on any
structure it cannot parse (fail closed). The subset parser below is
deliberately narrow: release.yml is under this repository's control,
and an unrecognized shape must fail loudly, not silently pass.
`--workflow PATH` points the checker at a different release workflow
file (used for negative-control runs).
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_WORKFLOW = REPO / ".github" / "workflows" / "release.yml"
RUST_CI_WORKFLOW = REPO / ".github" / "workflows" / "rust-ci.yml"

# The repository's single pinned dtolnay/rust-toolchain revision (see
# rust-ci.yml; every workflow that installs Rust must use exactly this).
PINNED_TOOLCHAIN = "dtolnay/rust-toolchain@02cb101ec7c40f2c49e1d9714d64511d8e1b74de"
TOOLCHAIN_VERSION = "stable"

BUILD_JOBS = ("build-linux", "build-macos", "build-windows")
PUBLISH_JOB = "release"
CONTRACT_JOB = "contract"
CHECKER_INVOCATION = "scripts/check-release-workflow.py"
EXPOSE_STEP_NAME = "Expose runner rustup on PATH"

PLATFORM_TARGETS = {
    "build-macos": ("x86_64-apple-darwin", "aarch64-apple-darwin"),
    "build-windows": ("x86_64-pc-windows-gnu",),
}

# A cargo/rustc/rustup *command position*, not prose: preceded by line
# start or a shell separator. This keeps `$CARGO_HOME/bin/rustup` and
# quoted diagnostics in the PATH-expose step from counting as Rust
# commands while still catching every real invocation.
RUST_COMMAND = re.compile(r"(?:^|[\s;&(])(?:cargo|rustc|rustup)\b", re.M)
BUILD_COMMAND = re.compile(r"\bcargo (?:build|test|clippy|doc)\b")

PUBLISH_IF_CONDITIONS = (
    "github.repository_owner == 'aliasfoxkde'",
    "github.event_name == 'push'",
    "github.ref_type == 'tag'",
    "startsWith(github.ref, 'refs/tags/v')",
)

BLOCK_SCALAR_INDICATORS = {"|", ">", "|-", ">-", "|+", ">+"}


class ContractError(Exception):
    """The workflow file is not in a shape this checker can judge."""


# --- parsing (indentation-subset; anything else fails closed) ------------


def strip_comment_lines(text: str) -> str:
    return "\n".join(
        line for line in text.splitlines() if not line.lstrip().startswith("#")
    )


def strip_inline_comment(value: str) -> str:
    return re.sub(r"\s+#.*$", "", value).strip()


def split_header_and_jobs(text: str) -> tuple[str, dict[str, list[str]]]:
    """Split the workflow into (header, job bodies). Job keys are the
    two-space-indented mappings under `jobs:`; the next column-0 key
    ends the mapping."""
    header_lines: list[str] = []
    jobs: dict[str, list[str]] = {}
    current: str | None = None
    in_jobs = False
    for raw in text.splitlines():
        line = raw.rstrip()
        if not in_jobs:
            if re.fullmatch(r"jobs:\s*(#.*)?", line):
                in_jobs = True
                continue
            header_lines.append(raw)
            continue
        if line.lstrip().startswith("#"):
            # Comments carry no structure (the # ─── job banners); checks
            # that read run bodies strip them separately.
            continue
        match = re.fullmatch(r"  ([A-Za-z0-9_-]+):\s*(#.*)?", line)
        if match:
            current = match.group(1)
            jobs[current] = []
            continue
        if not line.strip():
            if current is not None:
                jobs[current].append(line)
            continue
        if re.match(r"\S", line):
            header_lines.append(raw)
            in_jobs = False
            current = None
            continue
        if current is None:
            raise ContractError(f"content under jobs: before any job key: {line!r}")
        jobs[current].append(line)
    if not in_jobs and not jobs:
        raise ContractError("could not locate the top-level `jobs:` mapping")
    return "\n".join(header_lines), jobs


def parse_flow_list(value: str) -> list[str]:
    inner = strip_inline_comment(value)
    if not (inner.startswith("[") and inner.endswith("]")):
        raise ContractError(f"expected flow list, got {value!r}")
    return [item.strip() for item in inner[1:-1].split(",") if item.strip()]


def job_key_value(body: list[str], key: str) -> str | None:
    """Scalar value of a job-level key (4-space indent); block scalars
    are folded on spaces."""
    for i, line in enumerate(body):
        match = re.fullmatch(rf"    {re.escape(key)}:\s*(.*)", line)
        if not match:
            continue
        inline = strip_inline_comment(match.group(1))
        if inline and inline not in BLOCK_SCALAR_INDICATORS:
            return inline
        parts = [
            following.strip()
            for following in body[i + 1 :]
            if following.strip() and len(following) - len(following.lstrip()) >= 6
        ]
        return " ".join(parts)
    return None


def job_env_entries(body: list[str]) -> list[str]:
    """Job-level `env:` mapping entries, as `KEY: value` strings."""
    entries: list[str] = []
    in_env = False
    for line in body:
        if re.fullmatch(r"    env:\s*(#.*)?", line):
            in_env = True
            continue
        if not in_env:
            continue
        if not line.strip():
            continue
        if len(line) - len(line.lstrip()) <= 4:
            in_env = False
            continue
        match = re.match(r"      ([A-Za-z_][A-Za-z0-9_]*):(.*)", line)
        if match:
            entries.append(f"{match.group(1)}: {match.group(2).strip()}")
    return entries


class Step:
    def __init__(self, job: str, index: int, block: list[str]):
        self.job = job
        self.index = index
        self.name = self._scalar(block, "name", marker=True)
        self.uses = self._scalar(block, "uses", marker=True)
        self.run = self._run(block)
        self.env = self._env(block)
        self.toolchain_input = self._scalar(block, "toolchain", indents=(10,))
        self.targets_input = self._scalar(block, "targets", indents=(10,))

    @staticmethod
    def _scalar(
        block: list[str], key: str, marker: bool = False, indents: tuple[int, ...] = (8,)
    ) -> str | None:
        # Step keys sit at 8 spaces; `with:` inputs at 10; marker-inline
        # keys on the `      - key:` line itself.
        patterns = [rf"^      - {re.escape(key)}:\s*(.*)"] if marker else []
        patterns += [
            "^" + " " * indent + re.escape(key) + r":\s*(.*)" for indent in indents
        ]
        for line in block:
            for pattern in patterns:
                match = re.match(pattern, line)
                if match:
                    return strip_inline_comment(match.group(1))
        return None

    @staticmethod
    def _run(block: list[str]) -> str:
        for i, line in enumerate(block):
            match = re.match(r"        run:\s*(.*)", line)
            if not match:
                continue
            inline = strip_inline_comment(match.group(1))
            if inline in BLOCK_SCALAR_INDICATORS:
                body = [
                    following[10:] if following.startswith(" " * 10) else following.strip()
                    for following in block[i + 1 :]
                    if following.strip()
                ]
                return "\n".join(body)
            if inline:
                return inline
        return ""

    @staticmethod
    def _env(block: list[str]) -> dict[str, str]:
        env: dict[str, str] = {}
        in_env = False
        for line in block:
            if re.fullmatch(r"        env:\s*(#.*)?", line):
                in_env = True
                continue
            if not in_env:
                continue
            match = re.match(r"          ([A-Za-z_][A-Za-z0-9_]*):\s*(.*)", line)
            if match:
                env[match.group(1)] = strip_inline_comment(match.group(2))
            elif line.strip():
                in_env = False
        return env

    def commands(self) -> str:
        """Run body with shell comment lines removed."""
        return strip_comment_lines(self.run)


def parse_steps(job: str, body: list[str]) -> list[Step]:
    needs = job_key_value(body, "needs")
    if needs and needs.startswith("-"):
        raise ContractError(
            f"{job}: block-form `needs:` sequences are not supported by this "
            "checker; use flow form (needs: [a, b])"
        )
    steps: list[Step] = []
    i = 0
    while i < len(body):
        line = body[i]
        if re.match(r"^ {6}- ", line):
            block = [line]
            j = i + 1
            while j < len(body):
                following = body[j]
                if not following.strip():
                    block.append(following)
                    j += 1
                    continue
                if re.match(r"^ {6}- ", following):
                    break
                if len(following) - len(following.lstrip()) >= 8:
                    block.append(following)
                    j += 1
                    continue
                break
            step = Step(job, index=len(steps), block=block)
            if not step.uses and not step.run and not step.name:
                raise ContractError(
                    f"{job}: unrecognized step shape at block start {line.strip()!r}"
                )
            if step.uses and step.run:
                raise ContractError(f"{job}: step has both `uses` and `run`")
            steps.append(step)
            i = j
        else:
            i += 1
    if not steps:
        raise ContractError(f"{job}: no steps found")
    return steps


# --- contract checks ------------------------------------------------------


def check_triggers(header: str, findings: list[str]) -> None:
    if not re.search(r"^  workflow_dispatch:\s*$", header, re.M):
        findings.append(
            "trigger workflow_dispatch is missing (the build/validation entry point)"
        )
    if not re.search(r"^    tags:\s*$", header, re.M) or not re.search(
        r"^      - 'v\*'\s*$", header, re.M
    ):
        findings.append("tag-push trigger for 'v*' is missing (release semantics)")


def check_toolchain_preamble(job: str, steps: list[Step], findings: list[str]) -> Step | None:
    """Every build job pins the repo toolchain exactly once, with the
    policy version declared, and no Rust command runs before the pin."""
    installs = [s for s in steps if s.uses and s.uses.split("@")[0] == "dtolnay/rust-toolchain"]
    if len(installs) != 1:
        findings.append(
            f"{job}: expected exactly one dtolnay/rust-toolchain step, found {len(installs)}"
        )
        return None
    install = installs[0]
    if install.uses != PINNED_TOOLCHAIN:
        findings.append(
            f"{job}: dtolnay/rust-toolchain is pinned to {install.uses!r}, "
            f"expected {PINNED_TOOLCHAIN!r}"
        )
    if install.toolchain_input != TOOLCHAIN_VERSION:
        findings.append(
            f"{job}: toolchain input is {install.toolchain_input!r}, "
            f"expected {TOOLCHAIN_VERSION!r}"
        )

    # The PATH-expose step is provisioning, not consumption — its diagnostic
    # prose mentions rustup, so exempt it from the command scan.
    first_rust_command = next(
        (
            s
            for s in steps
            if s.run and s.name != EXPOSE_STEP_NAME and RUST_COMMAND.search(s.commands())
        ),
        None,
    )
    if first_rust_command is not None and first_rust_command.index < install.index:
        findings.append(
            f"{job}: step {first_rust_command.name or first_rust_command.index + 1} runs a "
            "cargo/rustc/rustup command before the pinned toolchain install"
        )
    return install


def check_build_job(job: str, steps: list[Step], findings: list[str]) -> None:
    install = check_toolchain_preamble(job, steps, findings)
    if install is None:
        return

    if job == "build-linux":
        expose = next((s for s in steps if s.name == EXPOSE_STEP_NAME), None)
        if expose is None:
            findings.append(
                f"{job}: missing {EXPOSE_STEP_NAME!r} step (the self-hosted runner's job "
                "PATH omits $CARGO_HOME/bin; see rust-ci.yml)"
            )
        elif expose.index > install.index:
            findings.append(f"{job}: {EXPOSE_STEP_NAME!r} must precede the toolchain install")

        for step in steps:
            if step.run and BUILD_COMMAND.search(step.commands()):
                if step.env.get("TMPDIR") != "${{ runner.temp }}":
                    findings.append(
                        f"{job}: step {step.name or step.index + 1} runs a build command "
                        "without step-level TMPDIR: ${{ runner.temp }} "
                        "(quota-constrained /tmp on this runner)"
                    )

    for target in PLATFORM_TARGETS.get(job, ()):
        declared = (install.targets_input or "").replace(",", " ").split()
        if target not in declared:
            findings.append(
                f"{job}: toolchain install does not declare target {target} "
                f"(targets: {install.targets_input!r})"
            )

    first_build = next((s for s in steps if s.run and BUILD_COMMAND.search(s.commands())), None)
    version_check = next((s for s in steps if s.run and "cargo --version" in s.commands()), None)
    if first_build is None:
        findings.append(
            f"{job}: no cargo build/test step found (platform builds must not be skipped)"
        )
    elif version_check is None:
        findings.append(f"{job}: no `cargo --version` check before building")
    elif version_check.index > first_build.index:
        findings.append(f"{job}: the `cargo --version` check must precede the first build step")


def check_contract_job(steps: list[Step], findings: list[str]) -> None:
    if not any(s.run and CHECKER_INVOCATION in s.commands() for s in steps):
        findings.append(f"{CONTRACT_JOB}: no step runs {CHECKER_INVOCATION}")


def check_publish_job(body: list[str], steps: list[Step], findings: list[str]) -> None:
    condition = (job_key_value(body, "if") or "").strip()
    if not condition:
        findings.append(f"{PUBLISH_JOB}: publication job has no `if` gate")
    else:
        for required in PUBLISH_IF_CONDITIONS:
            if required not in condition:
                findings.append(
                    f"{PUBLISH_JOB}: publication gate is missing condition {required!r}"
                )

    needs = job_key_value(body, "needs") or ""
    declared = parse_flow_list(needs) if needs else []
    for required in (CONTRACT_JOB, *BUILD_JOBS):
        if required not in declared:
            findings.append(
                f"{PUBLISH_JOB}: does not depend on `{required}` (needs: {needs!r})"
            )

    uses_text = "\n".join(s.uses or "" for s in steps)
    for action in ("anchore/sbom-action@", "actions/attest@", "softprops/action-gh-release@"):
        if action not in uses_text:
            findings.append(f"{PUBLISH_JOB}: release payload action `{action}` is missing")
    if "SHA256SUMS" not in "\n".join(s.run for s in steps):
        findings.append(f"{PUBLISH_JOB}: checksum manifest generation is missing")
    permissions_text = "\n".join(body)
    for permission in ("contents: write", "id-token: write", "attestations: write"):
        if permission not in permissions_text:
            findings.append(f"{PUBLISH_JOB}: publish permission `{permission}` is missing")


def check_runner_context_env(job: str, body: list[str], findings: list[str]) -> None:
    """The PR #293 §8 rejection class: `${{ runner.* }}` at job-level env
    is unavailable there and invalidates the whole workflow file."""
    for entry in job_env_entries(body):
        if "${{ runner." in entry:
            findings.append(
                f"{job}: job-level env entry {entry!r} uses the runner context "
                "(move it to step-level env; see rust-ci.yml)"
            )


def check_rust_ci_pin_parity(text: str, findings: list[str]) -> None:
    pins = set(re.findall(r"uses:\s*(dtolnay/rust-toolchain@\S+)", text))
    if not pins:
        findings.append(f"{RUST_CI_WORKFLOW.name}: no dtolnay/rust-toolchain pin found")
    for pin in sorted(pins):
        if pin != PINNED_TOOLCHAIN:
            findings.append(
                f"{RUST_CI_WORKFLOW.name}: dtolnay pin {pin!r} disagrees with the "
                f"release workflow pin {PINNED_TOOLCHAIN!r}"
            )


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--workflow",
        type=Path,
        default=DEFAULT_WORKFLOW,
        help="release workflow file to check (default: %(default)s)",
    )
    args = ap.parse_args()

    findings: list[str] = []
    try:
        text = args.workflow.read_text()
        header, jobs = split_header_and_jobs(text)

        for required in (CONTRACT_JOB, *BUILD_JOBS, PUBLISH_JOB):
            if required not in jobs:
                findings.append(f"missing job `{required}`")

        check_triggers(header, findings)
        for job, body in jobs.items():
            check_runner_context_env(job, body, findings)

        if CONTRACT_JOB in jobs:
            check_contract_job(parse_steps(CONTRACT_JOB, jobs[CONTRACT_JOB]), findings)
        for job in BUILD_JOBS:
            if job in jobs:
                check_build_job(job, parse_steps(job, jobs[job]), findings)
        if PUBLISH_JOB in jobs:
            check_publish_job(jobs[PUBLISH_JOB], parse_steps(PUBLISH_JOB, jobs[PUBLISH_JOB]), findings)

        if RUST_CI_WORKFLOW.is_file():
            check_rust_ci_pin_parity(RUST_CI_WORKFLOW.read_text(), findings)

        summary = (
            f"jobs: {len(jobs)}, "
            f"build platforms: {sum(1 for job in BUILD_JOBS if job in jobs)}"
        )
        if findings:
            print(f"release workflow contract: FAILED ({summary}) — {args.workflow}")
            print(f"\nviolations: {len(findings)}")
            for finding in findings:
                print(f"  - {finding}")
            return 1
        print(f"release workflow contract: OK ({summary})")
        return 0
    except ContractError as error:
        print(f"release workflow contract: FAILED (unreadable structure) — {args.workflow}")
        print(f"  - {error}")
        return 1
    except OSError as error:
        print(f"release workflow contract: FAILED (unreadable file) — {args.workflow}")
        print(f"  - {error}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
