-- Add job execution metadata (image and structured steps)
-- This enables the scheduler → runner handoff to carry validated,
-- persisted pipeline/job execution metadata end-to-end without
-- falling back to placeholder commands or hardcoded images.

BEGIN;

-- Steps table: stores individual command steps for each job.
-- Allows the runner to execute structured steps with proper metadata
-- rather than parsing a JSON blob or using hardcoded commands.
CREATE TABLE job_steps (
    id          UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    job_id      UUID NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    step_index  INT NOT NULL,
    name        TEXT NOT NULL DEFAULT '',
    run         TEXT NOT NULL,
    env         JSONB NOT NULL DEFAULT '{}',
    working_dir TEXT,
    UNIQUE(job_id, step_index)
);

CREATE INDEX idx_job_steps_job ON job_steps(job_id);

-- Jobs table: add image column.
-- The container image to use for this job (e.g. "rust:1.75", "python:3.12").
-- Must be non-null for jobs in the queued/assigned/running states.
ALTER TABLE jobs ADD COLUMN image TEXT NOT NULL DEFAULT '';

COMMIT;
