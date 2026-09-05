# GitForge API container build and healthcheck receipt — 2026-09-05

## Scope

This receipt covers the `api-prod` Docker target on branch
`fix/api-healthcheck-curl`. It does not promote or restart any live GitForge
service.

## Fixes validated

- The API image now installs `curl`, which its Docker `HEALTHCHECK` invokes.
- Dockerfile crate paths now match the tracked `gitforge-*` workspace
  directories.
- Every workspace member declared in the root `Cargo.toml` is copied into the
  builder context.
- The builder uses `rust:1.98-bookworm`, which satisfies the locked
  dependency MSRV, and uses `cargo build --locked`.
- Docker stage aliases use canonical `AS` casing.

## Authoritative Fedora validation

Source was fetched from GitHub into a detached disposable worktree at:

```text
58f207b8d95e20e7223a8a3134f286ecdd1a072d
```

Command:

```text
docker build --target api-prod --tag gitforge-api-healthcheck:58f207b .
```

Result: exit 0. The resulting image was inspected before the temporary image
and worktree were removed:

```text
image=sha256:1238e1e3f8a2e2cc4ecc824738b0e323f52508ddd20ae96351313b328d53b97c
healthcheck=["CMD-SHELL","curl -f http://localhost:42780/health || exit 1"]
/usr/bin/curl
curl 7.88.1 (x86_64-pc-linux-gnu) libcurl/7.88.1
```

This proves image construction, healthcheck declaration, and healthcheck
binary presence. It does not prove that an API process responds successfully;
that requires a separately configured database-backed service smoke test.

## Earlier failures and resolution

1. The original Dockerfile referenced nonexistent `crates/gitforce-*`
   directories; corrected to `crates/gitforge-*`.
2. The workspace build then failed because several declared workspace members
   were omitted from the Docker build context; all members are now copied.
3. `rust:1.80-bookworm` could not parse the locked dependency graph; the
   builder was moved to Edition-2024-compatible Rust and then to 1.98 because
   the locked `sqlx 0.9` graph requires rustc 1.94 or newer.

## Promotion status

The branch is pushed for review. No live service was changed. The existing
untracked `artifacts/` directory in the canonical GitForge checkout was
preserved and excluded from the commit.
