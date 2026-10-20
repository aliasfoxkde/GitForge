#!/usr/bin/env python3
"""Print the list of affected workspace crates for the current change.

Outputs one crate name per line. Prints "ALL" when the change set cannot be
determined or touches the workspace manifest, so callers fail open to a full
run. Used by .gitforge.yml jobs to scope cargo invocations.
"""
import json
import os
import subprocess
import sys


def run(cmd):
    return subprocess.run(cmd, capture_output=True, text=True)


def main():
    base = os.environ.get("GIT_BASE", "HEAD")
    # changed files (staged + unstaged + untracked-ish via diff against base)
    r = run(["git", "diff", "--name-only", f"{base}..HEAD"])
    if r.returncode != 0:
        # fallback: diff against HEAD
        r = run(["git", "diff", "--name-only", "HEAD"])
    files = [ln.strip() for ln in r.stdout.splitlines() if ln.strip()]
    # include uncommitted
    r2 = run(["git", "diff", "--name-only"])
    for ln in r2.stdout.splitlines():
        if ln.strip() and ln.strip() not in files:
            files.append(ln.strip())
    r3 = run(["git", "diff", "--cached", "--name-only"])
    for ln in r3.stdout.splitlines():
        if ln.strip() and ln.strip() not in files:
            files.append(ln.strip())

    # Manifest changes => full run
    for f in files:
        if f in ("Cargo.toml", "Cargo.lock") or f.endswith("/Cargo.toml"):
            print("ALL")
            return 0

    # cargo metadata
    r = run(["cargo", "metadata", "--format-version=1", "--no-deps"])
    if r.returncode != 0:
        print("ALL")
        return 0
    meta = json.loads(r.stdout)
    packages = {p["name"]: p for p in meta.get("packages", [])}
    # map crate dir -> package name
    id_to_name = {p["id"]: p["name"] for p in meta.get("packages", [])}

    r = run(["cargo", "metadata", "--format-version=1"])
    if r.returncode != 0:
        print("ALL")
        return 0
    meta_full = json.loads(r.stdout)
    id_to_name = {p["id"]: p["name"] for p in meta_full.get("packages", [])}
    # resolve nodes
    nodes = meta_full.get("resolve", {}).get("nodes", [])
    deps = {name: set() for name in id_to_name.values()}
    rdeps = {name: set() for name in id_to_name.values()}
    for node in nodes:
        nid = node.get("id")
        name = id_to_name.get(nid)
        if not name:
            continue
        for dep in node.get("deps", []):
            dep_id = dep.get("pkg")
            dep_name = id_to_name.get(dep_id)
            if dep_name:
                deps.setdefault(name, set()).add(dep_name)
                rdeps.setdefault(dep_name, set()).add(name)

    # map file -> crate by path prefix
    def crate_for_file(f):
        f = f.replace("\\", "/")
        for prefix_root in ("crates/", "services/"):
            if f.startswith(prefix_root):
                seg = f[len(prefix_root):].split("/", 1)[0]
                cand = seg.replace("-", "_")
                if cand in deps:
                    return cand
                if seg in deps:
                    return seg
        return None

    changed = set()
    for f in files:
        c = crate_for_file(f)
        if c:
            changed.add(c)

    if not changed:
        # no Rust crate touched; nothing to scope (but don't print ALL; print nothing)
        return 0

    # reverse closure
    result = set(changed)
    queue = list(changed)
    while queue:
        c = queue.pop()
        for d in rdeps.get(c, ()):
            if d not in result:
                result.add(d)
                queue.append(d)

    for name in sorted(result):
        print(name)
    return 0


if __name__ == "__main__":
    sys.exit(main())
