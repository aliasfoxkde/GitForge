# GitForge API Documentation

REST API for the GitForge self-hosted Git platform with CI/CD capabilities.

## Base URL

```
http://localhost:8080
```

## Interactive Documentation

- **Swagger UI**: http://localhost:8080/swagger-ui
- **OpenAPI Spec**: http://localhost:8080/api-docs/openapi.json

## Authentication

GitForge uses JWT tokens for API authentication. Include the token in the Authorization header:

```
Authorization: Bearer <your-token>
```

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
GET /jobs/{id}
GET /jobs/{id}/logs
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
curl -X POST http://localhost:8080/repos \
  -H "Content-Type: application/json" \
  -d '{"name": "my-project", "visibility": "public"}'
```

### Check Pipeline Status

```bash
curl http://localhost:8080/pipeline-runs/your-run-id
```

### Register a Runner

```bash
curl -X POST http://localhost:8080/runners \
  -H "Content-Type: application/json" \
  -d '{"name": "docker-runner-1", "type": "docker", "capacity": 4}'
```

## Scheduler (Job Queue)

```
http://localhost:8081
```

The scheduler manages the lifecycle of CI/run-time jobs: enqueue, runner claim, completion, cancellation, and automatic reaping of stale claimed jobs.

### Base URL

```
http://localhost:8081
```

### Endpoints

#### Runners

```
POST /runners
POST /runners/:id/heartbeat
```

**Register Runner Request:**
```json
{
  "name": "docker-runner-1",
  "type": "docker",
  "capacity": 4
}
```

Supported runner `type` values: `docker`, `firecracker`, `bare-metal`.

**Register Runner Response:**
```json
{
  "id": "uuid",
  "name": "docker-runner-1",
  "type": "docker",
  "status": "online",
  "capacity": 4
}
```

---

#### Jobs

##### Enqueue a Job

```
POST /jobs
```

Creates a new pending job. The scheduler assigns it a UUID if `job_id` is not supplied.

**Request:**
```json
{
  "job_id": "optional-uuid",
  "pipeline_run_id": "optional-uuid",
  "repo_id": "optional-uuid",
  "name": "build",
  "commands": ["make build", "make test"],
  "working_dir": "/workspace/repo"
}
```

**Response** `201 Created` — returns the created job:
```json
{
  "job_id": "uuid",
  "name": "build",
  "pipeline_run_id": "uuid",
  "commands": ["make build", "make test"],
  "working_dir": "/workspace/repo"
}
```

##### Fetch Pending Jobs (Runner Claim)

```
GET /jobs/pending?runner_id=optional-uuid
```

Called by a runner to claim pending jobs. The scheduler atomically transitions each claimed job to `claimed` status in the durable store and returns the list.

**Response** `200 OK`:
```json
[
  {
    "job_id": "uuid",
    "name": "build",
    "pipeline_run_id": "uuid",
    "commands": ["make build", "make test"],
    "working_dir": "/workspace/repo"
  }
]
```

##### Get Job Status

```
GET /jobs/:id
```

Returns the current status of a job. For terminal jobs, returns the full `JobCompletion` record including `receipt_id` (when in durable mode).

**Non-terminal response:**
```json
{
  "job_id": "uuid",
  "status": "pending"
}
```

**Terminal response (succeeded):**
```json
{
  "job_id": "uuid",
  "status": "succeeded",
  "success": true,
  "exit_code": 0,
  "error": null,
  "completed_at": "2024-01-01T00:00:00Z",
  "receipt_id": "uuid"
}
```

**Terminal response (cancelled):**
```json
{
  "job_id": "uuid",
  "status": "cancelled",
  "success": false,
  "exit_code": -1,
  "error": "job cancelled",
  "completed_at": "2024-01-01T00:00:00Z",
  "receipt_id": "uuid"
}
```

##### Complete a Job

```
POST /jobs/:id/complete
```

Called by a runner to report a terminal outcome.

**Request:**
```json
{
  "success": true,
  "exit_code": 0,
  "error": null
}
```

**Response** `200 OK` — same `JobCompletion` shape as terminal status.

##### Cancel a Job

```
POST /jobs/:id/cancel
```

Cancels a pending or claimed job. Cancellation is idempotent and durable: the first terminal writer wins and mints an immutable `receipt_id`.

**Response** `200 OK`:
```json
{
  "job_id": "uuid",
  "status": "cancelled",
  "success": false,
  "exit_code": -1,
  "error": "job cancelled",
  "completed_at": "2024-01-01T00:00:00Z",
  "receipt_id": "uuid"
}
```

---

### Durable Job Status

A scheduler job transitions through the following statuses:

| Status | Terminal | Description |
|--------|----------|-------------|
| `pending` | No | Job is queued and awaiting a runner |
| `claimed` | No | A runner has claimed the job; execution in progress |
| `succeeded` | Yes | Job completed successfully |
| `failed` | Yes | Job completed with a non-zero exit code, or was reaped after lease expiry |
| `cancelled` | Yes | Job was explicitly cancelled |

A job is **terminal** when `status` is `succeeded`, `failed`, or `cancelled`. Terminal jobs are never re-assigned.

### Completion and Cancellation `receipt_id` Behavior

Every terminal transition mints an **immutable receipt identifier**:

- **`POST /jobs/:id/complete`**: On the winning durable transition, `receipt_id` is set to a new UUID. If the job is already terminal, the stored `receipt_id` is returned (first writer wins).
- **`POST /jobs/:id/cancel`**: Same semantics — the first terminal writer mints the receipt. Subsequent calls to cancel an already-terminal job return the stored receipt.
- **`GET /jobs/:id`**: For terminal jobs, returns the `receipt_id` if the job was completed via the durable path (i.e., when a `Pool` was configured). In in-memory mode (`create_state`), `receipt_id` is absent from the response.

The receipt is not a guarantee of success — both `succeeded` and `failed` outcomes receive receipts. It is a stable identifier for the terminal transition that can be used for deduplication.

### Claim Reaper Lifecycle Hook

The claim reaper is a background task that automatically recovers stale claimed jobs whose runners have disappeared.

The scheduler service starts it automatically using these environment variables:

| Variable | Default | Meaning |
|---|---:|---|
| `SCHEDULER_CLAIM_LEASE_SECS` | `300` | Age after which a claimed job is stale |
| `SCHEDULER_CLAIM_MAX_RETRIES` | `3` | Requeues before stale claims fail explicitly |
| `SCHEDULER_CLAIM_REAPER_INTERVAL_SECS` | `30` | Poll interval |

All values must be positive integers. The service performs an immediate reap
on startup and aborts the reaper during graceful shutdown.

**Function:** `start_claim_reaper(state, lease_secs, max_retries, poll_interval)`

**Parameters:**
- `lease_secs` (`i64`): Threshold in seconds after which a claimed job is considered stale (its runner hold expired).
- `max_retries` (`i32`): Maximum `requeue_count` before a stale job is transitioned to `failed` instead of requeued.
- `poll_interval` (`Duration`): How often the reaper checks for stale claims.

**Returns:** `Option<JoinHandle<()>>`
- `Some(handle)` when a durable pool is configured — the reaper task is running.
- `None` when the state was built with `create_state` (in-memory mode, no durable recovery).

**Behavior per stale job:**
1. If `requeue_count < max_retries`: atomically requeue the job (`status = pending`, clear `runner_id`/`claimed_at`, increment `requeue_count`). The job is eligible for re-assignment.
2. If `requeue_count >= max_retries`: transition to `failed` with `exit_code = -2` and `error = "reaped: lease expired and retry cap exceeded"`. A new `receipt_id` is minted. This is a terminal transition.

Terminal jobs (including those completed by a runner before the reaper runs) are skipped. The reaper runs an immediate bounded reap on startup before entering the interval loop.

**Typical initialization:**
```rust
let state = create_state_with_pool(scheduler, pool).await?;
if let Some(handle) = start_claim_reaper(&state, 60, 3, Duration::from_secs(30)) {
    // handle must be kept alive; dropping it aborts the task
}
```

## Rate Limits

No rate limits currently enforced in MVP.
