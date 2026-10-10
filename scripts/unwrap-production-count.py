#!/usr/bin/env python3
"""Categorize unwrap/expect/panic sites: production vs test-context.

A site is TEST-CONTEXT when it falls inside a `#[cfg(test)]` item
(module/fn/impl) at file scope. Everything else counts as production.
"""
import re
import sys

OUT = sys.stdout.write
PAT = re.compile(r"\.unwrap\(\)|\.expect\(|panic!\(")
CFG = re.compile(r"#\[cfg\(test\)\]")


def skip_raw_string(src, k):
    """k points just past an `r` (or the first `#` of `br#`). Return the
    index just past the closing quote-hashes, or -1 if not a raw string."""
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


def cfg_test_spans(src):
    """Spans (start, end) of top-level items preceded by #[cfg(test)]."""
    spans = []
    for m in CFG.finditer(src):
        # item begins after the attribute and any other attributes/doc lines
        i = m.end()
        while i < len(src) and src[i] in " \t\r\n":
            i += 1
        while i < len(src) and (src.startswith("#[", i) or src[i] == "/" or src[i] == "#"):
            if src.startswith("#[cfg(test)]", i):
                i += len("#[cfg(test)]")
                while i < len(src) and src[i] in " \t\r\n":
                    i += 1
                continue
            # skip line
            j = src.find("\n", i)
            i = j + 1 if j != -1 else len(src)
        # find the item's brace span
        brace = src.find("{", i)
        if brace == -1:
            continue
        depth = 0
        k = brace
        in_str = in_chr = in_lc = in_bc = False
        while k < len(src):
            c = src[k]
            nxt = src[k + 1] if k + 1 < len(src) else ""
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
                # raw string r"..." / r#"..."# — a quote inside it does not
                # open a normal string, and braces inside stay literal
                end = skip_raw_string(src, k + 1)
                if end != -1:
                    k = end - 1
            elif c == "'" and (k == 0 or not (src[k - 1].isalnum() or src[k - 1] == "_")):
                # char literal vs lifetime: a char literal has its closing
                # quote within a few chars ('a', '\n', '\\''); a lifetime
                # ('static) does not. Only enter escape-aware state on a
                # nearby close, else skip the quote as generic syntax.
                close = src.find("'", k + 1, k + 6)
                if close != -1:
                    in_chr = True
                    if src[close - 1] == "\\":
                        close = src.find("'", close + 1, close + 6)
                        if close == -1:
                            in_chr = False
                    if in_chr:
                        k = close  # bottom k += 1 lands past the literal
            elif c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
                if depth == 0:
                    break
            k += 1
        spans.append((m.start(), k))
    return spans


def main(paths):
    prod = test = 0
    prod_by_file = []
    for root in paths:
        for path in sorted(root.rglob("*.rs")):
            src = path.read_text(errors="replace")
            spans = cfg_test_spans(src)
            n_all = len(PAT.findall(src))
            n_test = sum(len(PAT.findall(src[a:b + 1])) for a, b in spans)
            # integration-test files (tests/ dirs) are test-context wholesale
            if "tests" in path.parts:
                test += n_all
                continue
            n_prod = n_all - n_test
            test += n_test
            if n_prod:
                prod += n_prod
                prod_by_file.append((n_prod, str(path)))
    for n, p in sorted(prod_by_file, reverse=True)[:15]:
        OUT(f"{n:5d}  {p}\n")
    OUT(f"\nproduction sites: {prod}\ntest-context sites: {test}\n")


if __name__ == "__main__":
    from pathlib import Path
    main([Path(p) for p in sys.argv[1:]])
