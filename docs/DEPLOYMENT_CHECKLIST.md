# GitForge Deployment Checklist

> **Version:** 2026-11-19
> **Purpose:** Verify deployment readiness before each release.

---

## Pre-Deployment

### 1. CI Gate Status

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` passes
- [ ] `cargo test --workspace --locked` passes
- [ ] All CI workflow checks are green

### 2. Migration Readiness

- [ ] `migrations/002_dead_letter.sql` has been reviewed
- [ ] Migration tested against staging PostgreSQL instance
- [ ] Migration is idempotent (`CREATE TABLE IF NOT EXISTS` — verified)
- [ ] Rollback plan documented: drop table if needed

### 3. Configuration

- [ ] `.env` updated with `GITFORGE_CI_TRIGGER_DEAD_LETTER_AFTER` (default: 172800 seconds = 48 hours)
- [ ] `GITFORGE_CI_TRIGGER_DEAD_LETTER_AFTER` value appropriate for deployment scale
- [ ] `GITFORGE_CANCELLATION_WATCH_FAILURES` configured if different from default (3)

---

## Deployment Steps

### Step 1: Apply Database Migration

```bash
psql "$DATABASE_URL" < migrations/002_dead_letter.sql
```

Verify:
```bash
psql "$DATABASE_URL" -c "SELECT id, payload, created_at FROM ci_trigger_dead_letter LIMIT 1;"
```

### Step 2: Update Docker Compose

Ensure `GITFORGE_CI_TRIGGER_DEAD_LETTER_AFTER` is present in `docker-compose.yml` under git-server service.

### Step 3: Restart Services (in order)

1. `docker-compose restart git-server` — new code picks up `dead_letter_pool`
2. `docker-compose restart api` — openapi.json loading from file
3. `docker-compose restart ci` — CI engine restart

### Step 4: Verify

- [ ] `docker-compose ps` shows all services healthy
- [ ] `docker-compose logs git-server | grep -i "starting"` confirms startup
- [ ] Swagger UI at `/swagger-ui` loads without errors
- [ ] Dead letter table exists in database

---

## Post-Deployment Monitoring

### Key Metrics to Watch

| Metric | Threshold | Alert |
|--------|-----------|-------|
| Dead letter entries accumulating | > 0 entries after 1h | Warning |
| Dead letter purge failures | Any in logs | Warning |
| CI trigger background retries | > 10 per hour | Warning |
| Dead letter table size | > 10,000 entries | Critical |

### Manual Checks

```bash
# Check for permanently failed triggers
psql "$DATABASE_URL" -c "SELECT count(*), last_error FROM ci_trigger_dead_letter GROUP BY last_error;"

# Check dead letter purge (runs hourly, check last run)
psql "$DATABASE_URL" -c "SELECT id, created_at, retry_count, last_error FROM ci_trigger_dead_letter ORDER BY created_at DESC LIMIT 10;"
```

---

## Rollback

If issues arise, reverse in order:

1. Restart services without new code
2. If migration causes issues:
   ```sql
   DROP TABLE IF EXISTS ci_trigger_dead_letter;
   ```
3. Restore `.env` to previous state
4. Re-deploy previous release binary

---

## Known Differences from Previous Version

| Component | Before | After |
|-----------|--------|-------|
| CI trigger retry | Unbounded background | Bounded by `GITFORGE_CI_TRIGGER_DEAD_LETTER_AFTER` (default 48h) |
| Failed triggers | Lost silently | Written to `ci_trigger_dead_letter` table |
| OpenAPI spec | Inline `json!()` macro | External `openapi.json` loaded from file |
| Cancellation watch | Hardcoded 3 failures | Configurable via `GITFORGE_CANCELLATION_WATCH_FAILURES` |
| Revision resolver | Allowed `{`, `}`, `:` | Rejects reflog and pathspec dangerous syntax |
| AI provider methods | No `#[must_use]` | `#[must_use]` on all `Result`-returning pub fn |
| CAS store methods | No `#[must_use]` | `#[must_use]` on all `Result`-returning pub fn |
