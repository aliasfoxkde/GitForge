#!/usr/bin/env python3
"""Fail-closed checks for the repository's active CI policy.

This intentionally uses only the Python standard library so it can run before
the Rust toolchain or third-party linters are installed.
"""
from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
WORKFLOW_ROOTS = (ROOT / ".github" / "workflows", ROOT / ".github" / "actions" / "templates")
USES_RE = re.compile(r"^\s*(?:-\s*)?uses:\s*([^\s#]+)")
SHA_RE = re.compile(r"^[0-9a-fA-F]{40}$")


def workflow_files() -> list[pathlib.Path]:
    return sorted(
        path
        for directory in WORKFLOW_ROOTS
        if directory.is_dir()
        for path in directory.rglob("*")
        if path.suffix in {".yml", ".yaml"} and not path.name.endswith(".disabled")
    )


def check_file(path: pathlib.Path) -> list[str]:
    errors: list[str] = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        match = USES_RE.match(line)
        if not match:
            continue
        reference = match.group(1)
        if reference.startswith("./"):
            continue
        if "@" not in reference:
            errors.append(f"{path.relative_to(ROOT)}:{line_number}: action has no immutable ref: {reference}")
            continue
        action, ref = reference.rsplit("@", 1)
        if not action or not SHA_RE.fullmatch(ref):
            errors.append(f"{path.relative_to(ROOT)}:{line_number}: action ref is not a 40-hex SHA: {reference}")
    return errors


def main() -> int:
    files = workflow_files()
    if not files:
        print("CI policy failed: no active workflow/template files found", file=sys.stderr)
        return 1
    errors = [error for path in files for error in check_file(path)]
    if errors:
        print("CI policy failed:")
        print("\n".join(errors))
        return 1
    print(f"CI policy passed: {len(files)} active workflow/template files have immutable action refs")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
