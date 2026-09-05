# GitForge API Documentation

REST API for the GitForge self-hosted Git platform with CI/CD capabilities.

## Base URL

```
http://localhost:42780
```

## Interactive Documentation

- **Swagger UI**: http://localhost:42780/swagger-ui
- **OpenAPI Spec**: http://localhost:42780/api-docs/openapi.json

## Authentication

Protected `/api` routes pass through a shared JWT authentication boundary and
receive an `AuthenticatedUser` context. Resource authorization is still
enforced by the individual route. The one documented bootstrap exception is
`POST /api/runners`, which is intentionally unauthenticated so a new runner
can register before receiving credentials; runner listing and detail routes
remain protected.

Repository routes expose only repositories owned by the authenticated user,
with `admin` and `maintainer` role overrides. Artifact routes require the
shared authenticated context and resolve artifact access through the owning
job, pipeline run, and repository; unauthorized artifacts are returned as
not-found to avoid leaking private resource existence.

Administrators may change a user's persisted role with
`PATCH /api/users/{id}/role`. Supported roles are `admin`, `maintainer`,
`developer`, and `read_only`; non-administrators receive `403`, invalid roles
receive `400`, and the last administrator cannot be demoted. Protected
requests resolve the current persisted role, so demotion takes effect
immediately even when an older JWT is presented.

GitForge uses JWT tokens for API authentication. Include the token in the Authorization header:

```
Authorization: Bearer <your-token>
```

The public authentication endpoints are mounted at the server root, not under
the protected `/api` prefix:

```
POST /auth/login
GET /auth/status
```

`POST /auth/login` accepts `username` and `password` and returns `token`,
`token_type`, and `expires_in`. `GET /auth/status` returns an
`authenticated` boolean and, for a valid bearer token, the user identity and
role. User registration is not exposed by the GitForge API; users must be
provisioned through the supported local CLI bootstrap path on a fresh
database. Run `gitforge admin --bootstrap --username <name> --email <email>
--confirm`; the password is read from the terminal and the command refuses to
run after an administrator exists.

### Endpoints

#### Health Check

```
GET /health
```

Returns server health status including database connectivity.

**Response:**
```json
{
  "status": "healthy",
  "timestamp": "2024-01-01T00:00:00Z",
  "database": "connected"
}
```

#### Repositories

```
GET /repos
POST /repos
GET /repos/{id}
DELETE /repos/{id}
```

#### User roles

```
PATCH /users/{id}/role
```

**Request:**
```json
{"role": "maintainer"}
```

**Create Repository Request:**
```json
{
  "name": "my-repo",
  "visibility": "private"
}
```

**Response:**
```json
{
  "id": "uuid",
  "name": "my-repo",
  "owner_id": "user-uuid",
  "visibility": "private",
  "git_path": "/git/my-repo",
  "created_at": "2024-01-01T00:00:00Z",
  "updated_at": "2024-01-01T00:00:00Z"
}
```

#### Pipelines

```
GET /pipelines
GET /pipelines/{id}
```

#### Pipeline Runs

```
GET /pipeline-runs
GET /pipeline-runs/{id}
GET /pipeline-runs/{id}/jobs
```

**Pipeline Run Response:**
```json
{
  "id": "uuid",
  "pipeline_id": "uuid",
  "status": "running",
  "commit_hash": "abc123",
  "triggered_by": "push",
  "started_at": "2024-01-01T00:00:00Z",
  "finished_at": null
}
```

#### Jobs

```
POST /jobs
GET /jobs/{id}
GET /jobs/{id}/logs
POST /jobs/{id}/cancel
```

Job submission requires a pipeline run owned by the authenticated user (or an
admin/maintainer role), bounded commands, and a stable `idempotency_key`.
Retries return the original job ID; reusing a key for different parameters is
rejected. Cancellation is persisted in the shared database so the separate CI
scheduler and runner observe it safely.

**Submit Job Request:**
```json
{
  "pipeline_run_id": "run-uuid",
  "name": "manual-check",
  "commands": ["cargo test"],
  "working_dir": null,
  "idempotency_key": "attempt-uuid"
}
```

**Job Response:**
```json
{
  "id": "uuid",
  "name": "build",
  "status": "running",
  "runner_id": "runner-uuid",
  "started_at": "2024-01-01T00:00:00Z",
  "finished_at": null
}
```

**Job Logs Response:**
```json
{
  "job_id": "uuid",
  "logs": "Building...\nRunning tests...\nAll tests passed!"
}
```

#### Code Review Runs

```
POST /review-runs
GET /review-runs/{id}
GET /review-runs/{id}/findings
```

Review run submission persists an AI code-review run for a repository head
(the review provider itself executes in a separate worker; runs stay `pending`
until dispatch). Submission requires the repository to be owned by the
authenticated user (or an admin/maintainer role) and validates bounded
`base_sha`, `head_sha`, `idempotency_key`, and a positive `attempt`. The
repository is identified by `repo_id`, or by `owner` plus `name` (exactly one
form must be supplied); unknown repositories return `404` before any run is
created.

Idempotency follows the review contract: the first submission creates the run
(`201`), a retry with the same key against the same head SHA returns the
existing run (`200`), and reuse of the same key against a different head SHA
is a `409` conflict that never silently re-points the run.

Findings are read back in deterministic order (path, then line with NULL lines
first, then fingerprint) and bounded pagination (`limit` 1-500, default 100;
`offset` >= 0). Runs the caller is not authorized to see are reported as
`404`, never `403`, to avoid leaking private review activity.

**Submit Review Run Request:**
```json
{
  "repo_id": "repo-uuid",
  "base_sha": "base-commit-sha",
  "head_sha": "head-commit-sha",
  "idempotency_key": "client-generated-key",
  "attempt": 1
}
```

**Submission Response (201 created / 200 idempotent retry):**
```json
{
  "status": "created",
  "run": {
    "id": "run-uuid",
    "repo_id": "repo-uuid",
    "base_sha": "base-commit-sha",
    "head_sha": "head-commit-sha",
    "idempotency_key": "client-generated-key",
    "status": "pending",
    "attempt": 1,
    "receipt_id": null,
    "created_at": "2026-09-05T00:00:00+00:00",
    "updated_at": "2026-09-05T00:00:00+00:00"
  }
}
```

**Findings Response:**
```json
{
  "run_id": "run-uuid",
  "run_status": "succeeded",
  "total": 4,
  "limit": 100,
  "offset": 0,
  "findings": [
    {
      "id": "finding-uuid",
      "run_id": "run-uuid",
      "source": "static-analysis",
      "fingerprint": "16-hex-digest",
      "path": "src/main.rs",
      "line": 42,
      "severity": "warning",
      "category": "logic",
      "title": "Off-by-one",
      "message": "Loop excludes the final element.",
      "evidence": null,
      "confidence": "high",
      "position_status": "line",
      "disposition": "pending",
      "created_at": "2026-09-05T00:00:00+00:00",
      "updated_at": "2026-09-05T00:00:00+00:00"
    }
  ]
}
```

#### Runners

```
GET /runners
POST /runners
GET /runners/{id}
```

**Register Runner Request:**
```json
{
  "name": "runner-1",
  "type": "docker",
  "capacity": 4
}
```

**Response:**
```json
{
  "id": "uuid",
  "name": "runner-1",
  "type": "docker",
  "status": "online",
  "capacity": 4,
  "last_heartbeat": "2024-01-01T00:00:00Z"
}
```

#### Artifacts

```
GET /artifacts
GET /artifacts/{id}
DELETE /artifacts/{id}
GET /jobs/{job_id}/artifacts
```

**Artifact Response:**
```json
{
  "id": "uuid",
  "job_id": "job-uuid",
  "name": "test-results.zip",
  "path": "/artifacts/test-results.zip",
  "checksum": "abc123",
  "size_bytes": 1024,
  "created_at": "2024-01-01T00:00:00Z"
}
```

## Metrics

```
GET /metrics
```

Prometheus-format metrics including:
- `gitforge_http_requests_total` - HTTP request counts
- `gitforge_job_duration_seconds` - Job execution duration
- `gitforge_runners_online` - Number of online runners
- `gitforge_pipeline_runs_total` - Pipeline run counts by status
- And more...

## Status Codes

| Code | Description |
|------|-------------|
| 200 | Success |
| 201 | Created |
| 204 | No Content (deleted) |
| 400 | Bad Request |
| 401 | Unauthorized |
| 404 | Not Found |
| 500 | Internal Server Error |

## Example Usage

### Create a Repository

```bash
curl -X POST http://localhost:42780/repos \
  -H "Content-Type: application/json" \
  -d '{"name": "my-project", "visibility": "public"}'
```

### Check Pipeline Status

```bash
curl http://localhost:42780/pipeline-runs/your-run-id
```

### Register a Runner

```bash
curl -X POST http://localhost:42780/runners \
  -H "Content-Type: application/json" \
  -d '{"name": "docker-runner-1", "type": "docker", "capacity": 4}'
```

## Rate Limits

No rate limits currently enforced in MVP.
