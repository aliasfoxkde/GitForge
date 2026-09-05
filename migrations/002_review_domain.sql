-- GitForce Database Schema
-- Review domain migration (review runs and findings)

-- Review Runs
CREATE TABLE review_runs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    repo_id UUID REFERENCES repositories(id) ON DELETE SET NULL,
    base_sha TEXT NOT NULL,
    head_sha TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE CHECK (length(idempotency_key) <= 128 AND length(idempotency_key) > 0),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'running', 'succeeded', 'failed', 'cancelled')),
    attempt INT NOT NULL DEFAULT 1,
    receipt_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_review_runs_repo ON review_runs(repo_id);
CREATE INDEX idx_review_runs_status ON review_runs(status);

-- Review Findings
CREATE TABLE review_findings (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    run_id UUID NOT NULL REFERENCES review_runs(id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    path TEXT NOT NULL,
    line INT,
    severity TEXT NOT NULL,
    category TEXT NOT NULL,
    title TEXT NOT NULL,
    message TEXT NOT NULL,
    evidence TEXT,
    confidence TEXT NOT NULL,
    position_status TEXT NOT NULL CHECK (position_status IN ('line', 'file', 'deleted', 'unavailable')),
    disposition TEXT NOT NULL DEFAULT 'pending',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK ((position_status = 'line' AND line IS NOT NULL) OR (position_status <> 'line' AND line IS NULL)),
    UNIQUE (run_id, fingerprint)
);

CREATE INDEX idx_review_findings_run ON review_findings(run_id);
