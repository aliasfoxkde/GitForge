#!/usr/bin/env python3
"""Route-vs-docs drift checker for the API gateway.

Cross-checks three surfaces that hand-maintenance lets drift apart:

  1. Routes declared in the axum routers (`.route("/path", get(..)..)`)
     under crates/gitforge-api/src/ — the served truth.
  2. Paths in the hand-written OpenAPI spec (crates/gitforge-api/src/
     openapi.rs "paths" object) — the machine-readable contract.
  3. Endpoints documented in docs/API.md — the human contract.

A route that exists in (1) but not in (2)/(3) is undocumented surface;
an entry in (2)/(3) with no backing route is stale documentation. Both
directions fail the check.

Parse-only: no cargo, no network. `#[cfg(test)]` items are excluded via
brace-span matching (same lexer as unwrap-production-count.py, which is
raw-string/char-literal aware) so test-only routers don't pollute the
source set. Exit 0 = clean, 1 = drift. `--report` lists drift but
always exits 0 (for baseline runs while findings are being fixed).

Prefix convention: the app mounts auth/public routes at the root and
nest()s protected routes under /api (services/api server.rs). The
checker cannot always resolve which router a fragment lands in, so a
fragment matches an entry verbatim OR with the /api prefix added OR
stripped — lenient in the direction that avoids false "missing"
reports. Path parameters are normalized to `{}` since surfaces
disagree on names (`{id}` vs `{job_id}`).
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SRC_ROOT = REPO / "crates" / "gitforge-api" / "src"
OPENAPI_RS = SRC_ROOT / "openapi.rs"
API_MD = REPO / "docs" / "API.md"

CFG = re.compile(r"#\[cfg\(test\)\]")
METHODS = "(?:get|post|put|patch|delete|head|options|trace)"
ROUTE_HEAD = re.compile(r'\.route\(\s*("(?:[^"\\]|\\.)*")\s*,')
DOC_LINE = re.compile(r"^(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS|TRACE)\s+(/\S+?)\s*$", re.M)
DOC_INLINE = re.compile(r"`(?:GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS|TRACE)\s+(/[^`\s]+)`")
METHOD_WORD = re.compile(r"[.(]" + METHODS + r"\(")

# Meta-endpoints that serve the API's own documentation (swagger UI and
# the spec JSON). docs/API.md references them as URLs in its
# "Interactive Documentation" section, not as METHOD-endpoint entries,
# and an OpenAPI spec does not document its own delivery endpoint.
# Excluded so the comparison stays on the REST surface itself.
META_ROUTES = {"/swagger-ui", "/api-docs/openapi.json"}


# --- lexer (shared with unwrap-production-count.py) ---------------------


def skip_raw_string(src: str, k: int) -> int:
    """k points just past an `r`. Return index past the closing
    quote-hashes, or -1 if not a raw string."""
    hashes = 0
    j = k
    while j < len(src) and src[j] == "#":
        hashes += 1
        j += 1
    if j >= len(src) or src[j] != '"':
        return -1
    j += 1
    end = src.find('"' + "#" * hashes, j)
    return (end + hashes + 1) if end != -1 else len(src)


def blank_comments(src: str) -> str:
    """Blank comment contents (offsets preserved). String literals are
    KEPT — route paths live in them."""
    out = list(src)
    k, n = 0, len(src)
    while k < n:
        c = src[k]
        nxt = src[k + 1] if k + 1 < n else ""
        if c == "/" and nxt == "/":
            j = src.find("\n", k)
            j = n if j == -1 else j
            for i in range(k, j):
                if out[i] != "\n":
                    out[i] = " "
            k = j
        elif c == "/" and nxt == "*":
            j = src.find("*/", k + 2)
            j = n if j == -1 else j + 2
            for i in range(k, j):
                if out[i] != "\n":
                    out[i] = " "
            k = j
        elif c == '"':
            j = k + 1
            while j < n:
                if src[j] == "\\":
                    j += 1
                elif src[j] == '"':
                    break
                j += 1
            k = j + 1
        elif c == "r" and (k == 0 or not (src[k - 1].isalnum() or src[k - 1] == "_")):
            end = skip_raw_string(src, k + 1)
            k = end if end != -1 else k + 1
        else:
            k += 1
    return "".join(out)


def cfg_test_spans(src: str) -> list[tuple[int, int]]:
    """Spans (start, end) of items preceded by #[cfg(test)]."""
    spans = []
    n = len(src)
    for m in CFG.finditer(src):
        i = m.end()
        while i < n and src[i] in " \t\r\n":
            i += 1
        while i < n and (src.startswith("#[", i) or src[i] in "/#"):
            if src.startswith("#[cfg(test)]", i):
                i += len("#[cfg(test)]")
                while i < n and src[i] in " \t\r\n":
                    i += 1
                continue
            j = src.find("\n", i)
            i = j + 1 if j != -1 else n
        brace = src.find("{", i)
        if brace == -1:
            continue
        depth = 0
        k = brace
        in_str = in_chr = in_lc = in_bc = False
        while k < n:
            c = src[k]
            nxt = src[k + 1] if k + 1 < n else ""
            if in_lc:
                if c == "\n":
                    in_lc = False
            elif in_bc:
                if c == "*" and nxt == "/":
                    in_bc = False
                    k += 1
            elif in_str:
                if c == "\\":
                    k += 1
                elif c == '"':
                    in_str = False
            elif in_chr:
                if c == "\\":
                    k += 1
                elif c == "'":
                    in_chr = False
            elif c == "/" and nxt == "/":
                in_lc = True
                k += 1
            elif c == "/" and nxt == "*":
                in_bc = True
                k += 1
            elif c == '"':
                in_str = True
            elif c == "r" and (k == 0 or not (src[k - 1].isalnum() or src[k - 1] == "_")):
                end = skip_raw_string(src, k + 1)
                if end != -1:
                    k = end - 1
            elif c == "'":
                # char literal vs lifetime: a char literal has its closing
                # quote within a few chars; a lifetime does not.
                close = src.find("'", k + 1, k + 6)
                if close != -1:
                    in_chr = True
                    if src[close - 1] == "\\":
                        close = src.find("'", close + 1, k + 8)
                        if close == -1:
                            in_chr = False
                    if in_chr:
                        k = close
            elif c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
                if depth == 0:
                    break
            k += 1
        spans.append((m.start(), k))
    return spans


def strip_test_items(src: str) -> str:
    out = list(src)
    for a, b in cfg_test_spans(src):
        for i in range(a, min(b, len(out))):
            if out[i] != "\n":
                out[i] = " "
    return "".join(out)


def group_span(src: str, open_idx: int) -> int:
    """Index just past the brace/paren group opened at open_idx."""
    opener = src[open_idx]
    closer = "}" if opener == "{" else ")"
    depth = 0
    k = open_idx
    while k < len(src):
        if src[k] == opener:
            depth += 1
        elif src[k] == closer:
            depth -= 1
            if depth == 0:
                return k + 1
        k += 1
    return len(src)


# --- extraction ----------------------------------------------------------


def norm(path: str) -> str:
    path = path.strip()
    if not path.startswith("/"):
        return path
    path = re.sub(r"\{[^}]*\}", "{}", path)
    return path.rstrip("/") or "/"


def extract_source_routes() -> dict[str, set[str]]:
    """normalized path -> set of method names seen."""
    routes: dict[str, set[str]] = {}
    for rs in sorted(SRC_ROOT.rglob("*.rs")):
        code = blank_comments(strip_test_items(rs.read_text()))
        for m in ROUTE_HEAD.finditer(code):
            raw = m.group(1)[1:-1]  # strip quotes
            end = group_span(code, m.end() - 1)
            body = code[m.end():end]
            methods = set(METHOD_WORD.findall(body))
            if not methods:
                methods = {"?"}
            key = norm(raw)
            routes.setdefault(key, set()).update(methods)
    return routes


def extract_openapi_paths() -> set[str]:
    src = blank_comments(OPENAPI_RS.read_text())
    m = re.search(r'"paths"\s*:\s*\{', src)
    if not m:
        return set()
    end = group_span(src, m.end() - 1)
    body = src[m.end():end]
    # A path key opens directly into a method object ("/x": { "get": ...
    # ); requiring that method object is what keeps nested keys like
    # "parameters"/"responses" from matching.
    keys = re.findall(
        r'"(/[^"]*)"\s*:\s*\{\s*"(?:'
        + METHODS
        + r")\"\s*:\s*\{",
        body,
    )
    return {norm(k) for k in keys}


def extract_doc_endpoints() -> set[str]:
    paths: set[str] = set()
    text = API_MD.read_text()
    for m in DOC_LINE.finditer(text):
        paths.add(norm(m.group(2)))
    for m in DOC_INLINE.finditer(text):
        paths.add(norm(m.group(1)))
    return paths


def covered(path: str, entries: set[str]) -> bool:
    """True if any entry matches path verbatim, /api-prefixed, or
    /api-stripped (bidirectional because the mounting point of a
    fragment — root vs nested — is not resolved statically)."""
    return (
        path in entries
        or ("/api" + path) in entries
        or (path.startswith("/api/") and path[4:] in entries)
    )


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--report",
        action="store_true",
        help="list drift but always exit 0",
    )
    args = ap.parse_args()

    routes = extract_source_routes()
    spec_paths = extract_openapi_paths()
    doc_paths = extract_doc_endpoints()

    problems: list[str] = []

    for path in sorted(routes):
        if path in META_ROUTES:
            continue
        if not covered(path, spec_paths):
            problems.append(f"route {path}: no OpenAPI path entry")
        if not covered(path, doc_paths):
            problems.append(f"route {path}: not documented in docs/API.md")

    for path in sorted(spec_paths):
        if not covered(path, set(routes)):
            problems.append(f"OpenAPI path {path}: no backing route")

    for path in sorted(doc_paths):
        if not covered(path, set(routes)):
            problems.append(f"docs/API.md endpoint {path}: no backing route")

    print(
        f"source routes: {len(routes)}, openapi paths: {len(spec_paths)}, "
        f"doc endpoints: {len(doc_paths)}"
    )
    if problems:
        print(f"\ndrift: {len(problems)} finding(s)")
        for p in problems:
            print(f"  - {p}")
        return 0 if args.report else 1
    print("no drift")
    return 0


if __name__ == "__main__":
    sys.exit(main())
