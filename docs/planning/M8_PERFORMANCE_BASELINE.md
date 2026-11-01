# M8 Performance Baseline (2026-11-01)

Characterization of the live GitForge gateway API on the local instance
(`http://127.0.0.1:42780`, SWARMONE host, 16 cores, kernel
6.12.107+deb13-amd64), produced by `perfprobe` (`crates/gitforge-perfprobe`,
commit `b8d7efe3` on `feat/m8-observability`).

## Methodology

- Harness: `perfprobe baseline` — sequential per-endpoint batches of GET
  requests, fixed concurrency via a semaphore, per-request latency measured
  client-side.
- Endpoints: `/health`, `/metrics`, `/api/repos`, `/api/pipeline-runs`
  (the latter two authed via `/auth/login` bearer token).
- Levels: concurrency 1, 4, 16; 60 requests per endpoint per level.
- Zero errors at all levels (all responses 2xx).
- Raw JSON: `/var/tmp/m8d/baseline-c{1,4,16}.json` (host-local, ephemeral).

## Results

### concurrency = 1

| endpoint | p50 ms | p95 ms | p99 ms | max ms | rps |
|---|---|---|---|---|---|
| /health | 0.2 | 0.4 | 0.5 | 0.6 | 3310 |
| /metrics | 0.1 | 0.2 | 0.4 | 0.4 | 5152 |
| /api/repos | 0.6 | 0.8 | 1.0 | 1.3 | 1450 |
| /api/pipeline-runs | 1.3 | 1.8 | 2.0 | 2.1 | 686 |

### concurrency = 4

| endpoint | p50 ms | p95 ms | p99 ms | max ms | rps |
|---|---|---|---|---|---|
| /health | 0.3 | 0.5 | 0.5 | 0.6 | 11552 |
| /metrics | 0.2 | 0.3 | 0.3 | 0.3 | 18258 |
| /api/repos | 1.2 | 1.4 | 1.5 | 1.5 | 3113 |
| /api/pipeline-runs | 3.4 | 27.7 | 28.2 | 28.9 | 773 |

### concurrency = 16

| endpoint | p50 ms | p95 ms | p99 ms | max ms | rps |
|---|---|---|---|---|---|
| /health | 0.7 | 1.3 | 1.4 | 1.9 | 18251 |
| /metrics | 0.5 | 0.8 | 0.8 | 0.8 | 24837 |
| /api/repos | 3.3 | 4.0 | 4.1 | 4.7 | 4590 |
| /api/pipeline-runs | 12.5 | 15.6 | 16.5 | 16.9 | 1235 |

## Observations

1. **`/api/pipeline-runs` dominates tail latency** at every level and is the
   only endpoint whose p95 (27.7 ms at c=4) exceeds its c=16 p95 — a
   bimodal tail consistent with SQLite pool-acquire waits under concurrent
   readers, exactly what the M8c pool-wait instrumentation was added to
   expose. Follow-up: correlate with `gitforge_db_pool_acquire` histograms
   from `/metrics` during a run.
2. **Scaling is healthy elsewhere**: /health and /metrics scale linearly to
   16 workers (18–25k rps) with sub-2 ms tails; /api/repos holds sub-5 ms
   p99 at c=16.
3. **Throughput plateau on pipeline-runs** (~700–1200 rps regardless of
   concurrency) suggests a serialized resource (pool writer or query) rather
   than CPU.

## Confounds

- Live shared instance: co-tenant CI activity and other sessions' API traffic
  were present during the run (not quiesced).
- 60 samples per point is small for p99; treat p99 as indicative.
- Client and server share the host (loopback), so absolute numbers exclude
  network variability but include scheduler interference.

## Reproduction

```bash
cargo build --release -p gitforge-perfprobe   # CARGO_TARGET_DIR=/var/tmp/gitforge-target TMPDIR=/tmp
set -a; . ~/.config/gitforge/mkinney-credentials.env; set +a
/var/tmp/gitforge-target/release/perfprobe \
  --base-url http://127.0.0.1:42780 \
  --username "$GITFORGE_USER" --password "$GITFORGE_PASSWORD" \
  --requests 60 --concurrency 16 baseline
```
