# GitForge Authentication & Identity — Design Plan

> Status: **planned** — intentionally deferred until after the Amortyx
> release. This document exists so the build can start from a settled
> design instead of being re-derived under pressure.
> Written 2026-09-10, informed by the first real end-to-end use of the
> platform (the aegis release), which surfaced the gaps below.

## 1. Goals

1. **Email + password is the primary identity.** Usernames become display
   handles; email is the stable, unique key people actually remember.
2. **SSO in, not out.** Sign in with GitHub / Google / any generic OIDC
   provider; JIT-provision accounts; link existing local accounts.
3. **Zero-friction local mode.** `gitforge serve --local` runs with no
   account at all — one anonymous owner, limited features, upgrade path
   that keeps all data.
4. **Personal access tokens for git and CI.** Today git push and the CLI
   ride 24-hour JWTs (and the login CLI was broken until 2026-09-10 —
   `fix/cli-api-url-double-prefix`). Git automation needs long-lived,
   scoped, revocable tokens.
5. **Sane security defaults.** Argon2id hashing, login rate limiting,
   account lockout, audit events, and a real secret-management story
   (no more `JWT_SECRET`-only-in-one-process-environment).

## 2. Non-goals (for this phase)

- Organizations/teams RBAC (follows after identity is stable).
- SAML, LDAP, WebAuthn/passkeys (design leaves a hook; implementation later).
- Multi-instance federation.

## 3. Current state (as of 2026-09-10)

| Area | State |
|---|---|
| Accounts | `users(id, username UNIQUE, email UNIQUE, password_hash, role)`; roles `admin`, `developer` |
| Password hashing | bcrypt, `DEFAULT_COST` (cost 12) |
| Sessions | 24 h JWT, `JWT_SECRET` env var, no refresh, no revocation |
| Login | username + password only; CLI path was broken (double `/api` prefix, fixed in `fix/cli-api-url-double-prefix`) |
| Bootstrap | `gitforge admin --bootstrap` creates the first admin locally; refuses once any admin exists |
| Git transport | HTTP basic auth (username : JWT) on `:42782`; SSH on `:42022` |
| First-use experience | Owner must know to run the bootstrap; no signup; no invitation |
| Audit | `events` table exists (used by CI triggers); auth events not written |

Gaps observed in real use: password known by nobody after the provisioning
session ended; expired/rotated JWTs invalidate git automation; no
self-serve signup for new collaborators; no way to tell which services
share which secrets (see §11).

## 4. Identity model

- `users.username` stays (display + git namespace `<owner>/<repo>`), but
  login accepts **either** username or email in one field.
- `email` becomes the canonical identity link for SSO and resets.
- New columns:

```sql
ALTER TABLE users ADD COLUMN email_verified_at TIMESTAMPTZ;
ALTER TABLE users ADD COLUMN password_updated_at TIMESTAMPTZ;
ALTER TABLE users ADD COLUMN state TEXT NOT NULL DEFAULT 'active';
  -- active | locked | disabled
```

- Roles stay as-is (`admin`, `developer`) until org RBAC lands.

## 5. Password auth

- **Hash migration:** new hashes use Argon2id (19 MiB, t=2, p=1 as the
  default); verification keeps a bcrypt fallback and rehashes transparently
  on successful login. Cost of the sweep is one login per user.
- **Policy:** minimum 12 chars (already enforced by bootstrap), reject the
  top-10k common passwords (bundled list), no composition rules.
- **Reset:** `POST /auth/password/reset` issues a single-use,
  30-minute token (hashed at rest, like passwords). Email delivery is
  pluggable: SMTP config, or `--dev-print-token` for local runs.
- **Verification:** signup sends the same style of token; unverified email
  can log in but cannot reset password or own repos beyond the sandbox tier.
- **Lockout:** 10 failed attempts in 15 minutes locks for 15 minutes
  (exponential on repeat), writes an audit event. The existing
  `rate_limit.rs` middleware gains a per-IP+identifier bucket on
  `/auth/*`.

## 6. Sessions and tokens

Three token classes, clearly separated:

| Class | Lifetime | Use | Storage |
|---|---|---|---|
| Session JWT | 15 min | web/API requests | memory/cookie |
| Refresh token | 30 d, rotating | obtain new session JWTs | `refresh_tokens` table, hashed |
| Personal access token (PAT) | user-chosen or never | git push/pull, CLI, CI | `personal_access_tokens`, hashed, prefix `gf_` |

- PATs carry scopes: `repo:read`, `repo:write`, `ci:trigger`,
  `artifact:read`, `admin`. The git-server resolves a PAT to its scopes
  (currently it only checks existence — scopes are enforced at the API
  and by simple read/write mapping at git endpoints).
- Revocation: deleting a PAT or refresh token is immediate; JWTs die
  naturally in ≤15 min. A `jwt_revocations` kill-list covers
  "log out everywhere".
- `JWT_SECRET` moves from "one process's env var" to a root-only file
  (`/var/lib/gitforge/jwt.secret`, mode 0600 — the pattern already
  running in production since the 2026-09-10 restart) and all four
  services read the same file. Rotation: dual-key window.

```sql
CREATE TABLE refresh_tokens (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL REFERENCES users(id),
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    rotated_from UUID
);
CREATE TABLE personal_access_tokens (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL REFERENCES users(id),
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    scopes TEXT[] NOT NULL,
    last_used_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

## 7. SSO

- **Protocols:** OAuth2 (GitHub, Google) plus a generic OIDC discovery
  flow (`--oidc-issuer`, `client-id`, `client-secret`) so self-hosters can
  plug in Keycloak/Authentik/enterprise IdPs.
- **Flow:** authorization-code + PKCE; state cookie; callback at
  `/auth/oauth/{provider}/callback`.
- **JIT provisioning:** first login with a provider mints an account
  (email from the provider, username from login handle with
  dedup-suffixing) unless `SIGNUP_MODE=invite|approval` says otherwise.
- **Linking:** a signed short-lived `link_token` lets an already-authenticated
  local user attach a provider (`oauth_accounts` row) — never silently
  merge two accounts with different emails; when emails match, link after
  password confirmation.
- **Config:** providers come from config file or env
  (`GITFORGE_SSO_GITHUB_CLIENT_ID`, …); disabled providers are hidden.

```sql
CREATE TABLE oauth_accounts (
    provider TEXT NOT NULL,
    provider_account_id TEXT NOT NULL,
    user_id UUID NOT NULL REFERENCES users(id),
    email TEXT,
    linked_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (provider, provider_account_id)
);
```

## 8. Local / no-account mode

`gitforge serve --local` (or `GITFORGE_MODE=local`):

- Single implicit **anonymous owner**; no login screen; every request is
  the owner. Dashboard shows a persistent "local mode" banner.
- Limited features: private repos, CI, artifacts, releases — yes;
  sharing, SSH keys, multi-user, audit — no.
- Storage unchanged; the users table gets a sentinel row (`state='local'`).
- **Claim path:** `gitforge admin --bootstrap` (existing command) converts
  the anonymous owner into a real admin account and keeps every repo, run,
  and artifact. This is the exact hole the aegis release fell into —
  a session provisioned the instance, the credentials evaporated with the
  session, and recovery required local bootstrap.

## 9. Signup dynamics

`SIGNUP_MODE` config: `closed` (default; admin invites via
`POST /auth/invites`), `open` (public signups with email verification),
`approval` (signup queues for admin approval). Invitations are
single-use tokens with expiry; the instance owner chooses the posture.
Private-instance default is `closed`, which doubles as spam defense.

## 10. API and CLI surface

```
POST /auth/register                  # per SIGNUP_MODE
POST /auth/login                     # username OR email + password
POST /auth/password/reset            # request
POST /auth/password/reset/confirm    # consume token
GET  /auth/oauth/{provider}          # start SSO
GET  /auth/oauth/{provider}/callback
POST /auth/oauth/{provider}/link
GET/POST/DELETE /auth/tokens         # PAT management
POST /auth/invites                   # admin
POST /auth/logout-all
```

CLI: `gitforge auth login` gains `--email`; `gitforge token create/list/revoke`
manages PATs; the stored credential becomes a PAT (survives JWT rotation,
fixes the "automation dies every 24 h" problem for CI jobs and git pushes).

## 11. Secret management

- One root-owned dir `/var/lib/gitforge/secrets/` (0700): `jwt.secret`,
  SMTP creds, SSO client secrets. Services read files; env vars remain
  an override for containers.
- `gitforge secrets rotate` for the JWT key pair with a dual-key
  verification window.

## 12. Rollout plan (start after Amortyx ships)

1. **Phase 1 — foundations:** PATs + scopes + CLI `token` command; JWT
   lifetime down to 15 min + refresh; Argon2id migration. (Git transport
   stops depending on session JWTs.)
2. **Phase 2 — email identity:** email login, registration + verification,
   password reset, lockout/rate-limit hardening, audit events on `/auth/*`.
3. **Phase 3 — local mode + claim:** `--local` anonymous owner, bootstrap
   claim path, local-mode feature gating.
4. **Phase 4 — SSO:** GitHub + Google OAuth, generic OIDC, account linking,
   `SIGNUP_MODE` invites.

Each phase ships independently, nothing breaks existing `username+password`
logins, and every phase lands with integration tests against the sqlite
migration chain.

## 13. Open questions

- Email delivery for self-hosters without SMTP: bundle a tiny outbox that
  writes `.eml` files to disk for the operator to relay? (Default: yes.)
- Should PATs be usable at the SSH gate (§6) via a certificate-style
  prefix, or does SSH stay key-file only? (Lean: key-file only in phase 1.)
- Dashboard session cookie vs bearer for the web UI (needs a CSRF review
  of the existing dashboard routes before phase 2).
