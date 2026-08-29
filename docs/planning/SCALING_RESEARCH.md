# Git at Scale — Insights from Cursor's Architecture

> **Source:** [Cursor's "Git at Any Scale"](https://cursor.com/blog/git-at-any-scale#whats-hard-about-git)
> **Date:** 2026-08-28
> **Purpose:** Extract actionable insights for GitForge's roadmap

---

## Core Problem: Git's Distributed Architecture

Git's fundamental design—all replicas are identical—makes centralized hosting difficult. Packfiles, the basic storage unit, require filesystem access, preventing horizontal scalability.

### Key Quote
> "At every step of this walk, you don't know the value of the next pointer until you fetch the previous one."

This sequential dependency in Git's DAG structure means you can't parallelize object walks across distributed storage.

---

## Existing Approaches and Their Flaws

### Git without Packfiles
Content-addressable stores (CAS) map well to distributed key-value systems, but Git's DAG requires sequential object walks. No parallelism possible.

### Filesystem Distribution
Block-level replication (GFS, DRBD) fails because packfile layout has no correlation to on-disk organization. Random reads across gigabytes don't work over networked filesystems.

### GitHub's "Spokes" Architecture
Uses 3-phase commit for consistency across replicas. Two critical flaws:
1. **Latency bound by slowest server** — every step waits for all servers
2. **Repositories are "pets, not cattle"** — require external databases, routing tables, checksum validation

---

## Cursor's "Continuity" Solution

### Architecture
```
┌─────────────────────────────────────────────────────────────┐
│                      S3 (Source of Truth)                   │
│                  WAL + Packfiles (Append-only)              │
└─────────────────────────────────────────────────────────────┘
                              ▲
                              │ Replicate
                              │
┌─────────────────────────────────────────────────────────────┐
│                   Local NVMe (Warm Cache)                   │
│              Git Repositories on Fast Storage               │
└─────────────────────────────────────────────────────────────┘
```

### Key Properties
| Property | Implementation |
|----------|---------------|
| **Write Path** | WAL persisted to S3 first, local cache updated async |
| **Consistency** | Linearizable pushes — never acknowledge until persisted |
| **Routing** | Rendezvous hashing for soft routing — no routing tables |
| **Primary Election** | CAS operations on S3 — any server can be primary |
| **Replication** | UDP gossip + consistency verified via conditional S3 GETs |
| **Compaction** | Only primary compacts; replicas download pre-compacted packs |
| **Repository Identity** | "Where does every repository live? The answer is 'anywhere'" |

### Performance Results
- **100 replicas** with linear read scaling
- **S3 Standard:** 120 pushes/second
- **S3 Express One Zone:** 300+ pushes/second

---

## Key Insights for GitForge

### 1. Object Storage as Source of Truth

**Current GitForge:** SQLite database + filesystem Git storage

**Insight:** S3-compatible object storage provides:
- Infinite horizontal write scalability
- Built-in replication and durability
- CAS semantics for distributed consensus

**Recommendation:** Add `gitforge-object-store` crate with S3-compatible backend for:
- Pipeline WAL and audit logs
- Large artifact storage
- Git packfile backing store

### 2. Repository as Cache, Not Source of Truth

**Current GitForge:** Git repositories on local filesystem, SQLite as metadata store

**Insight:** Treat Git repositories as a warm cache materialized from persistent WAL/event log. This enables:
- Stateless git-server replicas
- Instant repository provisioning
- No "repository not found" errors on new replicas

**Recommendation:** Implement repository materialized views:
- Replica starts with empty local cache
- On-demand object fetch from S3 + local cache
- Background prefetch based on access patterns

### 3. Eliminating Routing Tables

**Current GitForge:** Explicit runner/repository assignments in database

**Insight:** Rendezvous hashing lets any server handle any repository. The question "where does this repo live?" becomes "anywhere."

**Recommendation:**
- Replace explicit runner-to-job assignments with consistent hashing
- Add soft-routing layer that discovers repository location dynamically
- Eliminate `runners` table routing entries

### 4. Fail-Write, Not Fail-Read

**Current GitForge:** Reads fail gracefully, writes are transactional

**Insight:** Design for "always correct when degraded, always fast when healthy"

**Recommendation:**
- On storage degradation: serve reads from cache, queue writes
- Add storage health endpoints with replica lag metrics
- Implement write-ahead buffering for transient failures

### 5. Write-Ahead Log for Operations

**Current GitForge:** Database transactions for state changes

**Insight:** WAL stored durably enables:
- Materialized repository state from log replay
- Point-in-time recovery
- Distributed consensus without distributed transactions

**Recommendation:** Implement operation log:
- `PushEvent`, `CloneEvent`, `GCEvent` persisted before processing
- Scheduler reads from WAL instead of database for runner assignment
- Enables event sourcing architecture

### 6. Compaction as Primary-Only Operation

**Current GitForge:** No distributed compaction strategy

**Insight:** Only the primary should perform packfile compaction. Replicas download pre-compacted packs, trading bandwidth for CPU.

**Recommendation:**
- Add primary election to git-server cluster
- Primary runs `git gc` on configured schedule
- Replicas sync via pre-packaged bundle downloads

---

## Roadmap Recommendations

### Phase 1: Storage Foundation (Near-term)
- [ ] Add `gitforge-object-store` crate with S3-compatible API
- [ ] Implement WAL persistence for push events
- [ ] Add S3 artifact storage backend
- [ ] Define object-store traits for pluggable backends (S3, GCS, MinIO, local)

### Phase 2: Distributed Git Server (Mid-term)
- [ ] Implement rendezvous hashing for repository routing
- [ ] Add primary election via CAS operations
- [ ] Implement replica sync via bundle downloads
- [ ] Add UDP gossip for cluster membership

### Phase 3: Cache-Forward Architecture (Long-term)
- [ ] Materialized repository views (replicas start empty, populate on demand)
- [ ] Background prefetch based on access patterns
- [ ] Point-in-time recovery from WAL replay
- [ ] Write-ahead buffering for degraded mode

### Phase 4: Elastic Scale (Future)
- [ ] Dynamic replica count based on traffic
- [ ] Cross-region replication
- [ ] S3 Express One Zone for ultra-low-latency writes
- [ ] 300+ pushes/second target

---

## Anti-Patterns to Avoid

Based on Cursor's analysis:

| Anti-Pattern | Problem | Solution |
|--------------|---------|----------|
| 3-phase commit | Latency bound to slowest replica | S3 CAS + eventual consistency |
| Routing tables | "Pets, not cattle" | Rendezvous hashing |
| Consensus algorithms | Scale poorly with replica count | Single primary, no voting |
| Block-level replication | Packfiles have no disk locality | Object storage + local cache |

---

## Summary

GitForge's architecture is well-suited for horizontal scale. The current SQLite + filesystem approach works for single-node deployments, but the insights from Cursor's article suggest clear paths to multi-node scalability:

1. **Object storage** as the source of truth enables infinite write scale
2. **Local repositories as cache** eliminates the "where does this repo live?" problem
3. **Rendezvous hashing** replaces routing tables with discovery
4. **WAL-first** architecture enables event sourcing and materialized views

The roadmap should prioritize storage abstraction in Phase 1, enabling all subsequent distributed improvements.
