-- Impact runs for change-scoped validation observability

CREATE TABLE IF NOT EXISTS impact_runs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    commit_sha TEXT NOT NULL,
    changed_files JSONB NOT NULL DEFAULT '[]'::jsonb,
    affected_crates JSONB NOT NULL DEFAULT '[]'::jsonb,
    affected_features JSONB NOT NULL DEFAULT '[]'::jsonb,
    tests_run JSONB NOT NULL DEFAULT '[]'::jsonb,
    duration_ms BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_impact_runs_commit ON impact_runs(commit_sha);
