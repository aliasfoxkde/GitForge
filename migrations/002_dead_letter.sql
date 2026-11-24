-- Migration 002: CI trigger dead letter table
--
-- When the database is permanently unavailable, CI trigger inserts that
-- would otherwise be lost go into the dead letter table instead of being
-- silently dropped or retried forever. A cleanup task purges entries
-- older than 30 days.

CREATE TABLE IF NOT EXISTS ci_trigger_dead_letter (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    payload TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    retry_count INT NOT NULL DEFAULT 0,
    last_error TEXT
);

CREATE INDEX idx_dead_letter_created ON ci_trigger_dead_letter(created_at DESC);
