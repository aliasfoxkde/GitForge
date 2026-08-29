//! Crash-safe local job journal for the build coordinator.

use crate::job::{BuildResult, JobStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Automatic compaction threshold for a local journal.
pub const DEFAULT_COMPACTION_BYTES: u64 = 16 * 1024 * 1024;

/// A durable state transition written before it is exposed as recovered state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JobJournalEvent {
    /// A new job and its exact argv were accepted.
    Submitted {
        job_id: uuid::Uuid,
        cargo_args: Vec<String>,
        executable: Option<String>,
        working_dir: Option<String>,
    },
    /// A worker acquired capacity and started the child process.
    Started {
        job_id: uuid::Uuid,
        pid: u32,
        process_group_id: i32,
        process_start_ticks: Option<u64>,
    },
    /// A child was reaped and its complete result is authoritative.
    Completed {
        job_id: uuid::Uuid,
        result: BuildResult,
    },
    /// The coordinator failed before obtaining a child result.
    Failed {
        job_id: uuid::Uuid,
        result: BuildResult,
    },
    /// An operator cancelled the job.
    Cancelled { job_id: uuid::Uuid },
}

/// The replayable state of one locally submitted job.
#[derive(Debug, Clone)]
pub struct RecoveredJob {
    pub job_id: uuid::Uuid,
    pub cargo_args: Vec<String>,
    pub executable: Option<String>,
    pub working_dir: Option<String>,
    pub status: JobStatus,
    pub result: Option<BuildResult>,
    pub process_group_id: i32,
    pub process_start_ticks: Option<u64>,
}

/// Result of replaying the journal.
#[derive(Debug, Default)]
pub struct JournalReplay {
    pub jobs: Vec<RecoveredJob>,
}

/// Append-only journal. Each record is one newline-delimited JSON object.
#[derive(Debug)]
pub struct JobJournal {
    path: PathBuf,
    file: File,
}

impl JobJournal {
    /// Open or create a journal, creating only its explicitly requested parent.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)?;
        // A process can die after writing only part of the final record. Trim
        // that record before appending so it cannot be concatenated with the
        // next valid event and hide both records during replay.
        let length = file.metadata()?.len();
        if length > 0 {
            let bytes = std::fs::read(&path)?;
            if bytes.last() != Some(&b'\n') {
                let truncate_at = bytes
                    .iter()
                    .rposition(|byte| *byte == b'\n')
                    .map(|index| index + 1)
                    .unwrap_or(0) as u64;
                file.set_len(truncate_at)?;
                file.seek(SeekFrom::End(0))?;
            }
        }
        Ok(Self { path, file })
    }

    /// Return the configured journal path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one complete record and force it to stable storage.
    pub fn append(&mut self, event: &JobJournalEvent) -> io::Result<()> {
        write_event(&mut self.file, event)?;
        if self.file.metadata()?.len() >= DEFAULT_COMPACTION_BYTES {
            self.compact()?;
        }
        Ok(())
    }

    /// Replace event history with a snapshot of the latest state of every
    /// job. The temporary file is synced before the rename so a crash cannot
    /// leave a partially written replacement at the journal path.
    pub fn compact(&mut self) -> io::Result<()> {
        let replay = self.replay()?;
        let temporary = self.path.with_extension("jsonl.compact");
        let mut replacement = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        for job in replay.jobs {
            write_event(
                &mut replacement,
                &JobJournalEvent::Submitted {
                    job_id: job.job_id,
                    cargo_args: job.cargo_args,
                    executable: job.executable,
                    working_dir: job.working_dir,
                },
            )?;
            match job.status {
                JobStatus::Queued => {}
                JobStatus::Running { pid } => write_event(
                    &mut replacement,
                    &JobJournalEvent::Started {
                        job_id: job.job_id,
                        pid,
                        process_group_id: job.process_group_id,
                        process_start_ticks: job.process_start_ticks,
                    },
                )?,
                JobStatus::Completed { .. } => {
                    if let Some(result) = job.result {
                        write_event(
                            &mut replacement,
                            &JobJournalEvent::Completed {
                                job_id: job.job_id,
                                result,
                            },
                        )?;
                    }
                }
                JobStatus::Failed { .. } => {
                    if let Some(result) = job.result {
                        write_event(
                            &mut replacement,
                            &JobJournalEvent::Failed {
                                job_id: job.job_id,
                                result,
                            },
                        )?;
                    }
                }
                JobStatus::Cancelled => write_event(
                    &mut replacement,
                    &JobJournalEvent::Cancelled { job_id: job.job_id },
                )?,
            }
        }
        replacement.sync_all()?;
        std::fs::rename(&temporary, &self.path)?;
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&self.path)?;
        Ok(())
    }

    /// Replay all complete records. A final partial line is treated as a
    /// crash-truncated record; malformed complete records are rejected.
    pub fn replay(&self) -> io::Result<JournalReplay> {
        let file = File::open(&self.path)?;
        let mut states: HashMap<uuid::Uuid, RecoveredJob> = HashMap::new();
        let lines: Vec<String> = BufReader::new(file).lines().collect::<Result<_, _>>()?;
        for (index, line) in lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let event: JobJournalEvent = match serde_json::from_str(line) {
                Ok(event) => event,
                Err(_error) if index + 1 == lines.len() => break,
                Err(error) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid journal record {}: {}", index + 1, error),
                    ));
                }
            };
            apply_event(&mut states, event);
        }
        Ok(JournalReplay {
            jobs: states.into_values().collect(),
        })
    }
}

fn write_event(file: &mut File, event: &JobJournalEvent) -> io::Result<()> {
    let record = serde_json::to_vec(event)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    file.write_all(&record)?;
    file.write_all(b"\n")?;
    file.sync_data()
}

fn apply_event(states: &mut HashMap<uuid::Uuid, RecoveredJob>, event: JobJournalEvent) {
    match event {
        JobJournalEvent::Submitted {
            job_id,
            cargo_args,
            executable,
            working_dir,
        } => {
            states.insert(
                job_id,
                RecoveredJob {
                    job_id,
                    cargo_args,
                    executable,
                    working_dir,
                    status: JobStatus::Queued,
                    result: None,
                    process_group_id: 0,
                    process_start_ticks: None,
                },
            );
        }
        JobJournalEvent::Started {
            job_id,
            pid,
            process_group_id,
            process_start_ticks,
        } => {
            if let Some(job) = states.get_mut(&job_id) {
                job.status = JobStatus::Running { pid };
                job.result = None;
                job.process_group_id = process_group_id;
                job.process_start_ticks = process_start_ticks;
            }
        }
        JobJournalEvent::Completed { job_id, result } => {
            if let Some(job) = states.get_mut(&job_id) {
                job.status = JobStatus::Completed {
                    exit_code: result.exit_code,
                    duration_ms: result.duration_ms,
                };
                job.result = Some(result);
            }
        }
        JobJournalEvent::Failed { job_id, result } => {
            if let Some(job) = states.get_mut(&job_id) {
                job.status = JobStatus::Failed {
                    exit_code: result.exit_code,
                    duration_ms: result.duration_ms,
                    error: result
                        .error
                        .clone()
                        .unwrap_or_else(|| "job failed".to_string()),
                };
                job.result = Some(result);
            }
        }
        JobJournalEvent::Cancelled { job_id } => {
            if let Some(job) = states.get_mut(&job_id) {
                job.status = JobStatus::Cancelled;
                job.result = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn replay_reconstructs_terminal_job() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.jsonl");
        let mut journal = JobJournal::open(&path).unwrap();
        let id = uuid::Uuid::new_v4();
        journal
            .append(&JobJournalEvent::Submitted {
                job_id: id,
                cargo_args: vec!["test".into()],
                executable: None,
                working_dir: Some("/workspace".into()),
            })
            .unwrap();
        let result = BuildResult {
            job_id: id,
            success: true,
            exit_code: 0,
            duration_ms: 12,
            output: crate::job::JobOutput {
                stdout: "ok".into(),
                stderr: String::new(),
                exit_code: 0,
            },
            error: None,
        };
        journal
            .append(&JobJournalEvent::Completed { job_id: id, result })
            .unwrap();
        let replay = journal.replay().unwrap();
        assert_eq!(replay.jobs.len(), 1);
        assert!(matches!(replay.jobs[0].status, JobStatus::Completed { .. }));
        assert!(replay.jobs[0].result.is_some());
    }

    #[test]
    fn replay_ignores_crash_truncated_final_record() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.jsonl");
        std::fs::write(&path, b"{\"Submitted\":").unwrap();
        let journal = JobJournal::open(&path).unwrap();
        assert!(journal.replay().unwrap().jobs.is_empty());
    }

    #[test]
    fn open_repairs_truncated_record_before_append() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.jsonl");
        std::fs::write(&path, b"{\"Submitted\":").unwrap();
        let mut journal = JobJournal::open(&path).unwrap();
        let id = uuid::Uuid::new_v4();
        journal
            .append(&JobJournalEvent::Submitted {
                job_id: id,
                cargo_args: vec!["--version".into()],
                executable: None,
                working_dir: None,
            })
            .unwrap();
        let replay = journal.replay().unwrap();
        assert_eq!(replay.jobs.len(), 1);
        assert_eq!(replay.jobs[0].job_id, id);
    }

    #[test]
    fn compact_preserves_latest_job_state() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.jsonl");
        let mut journal = JobJournal::open(&path).unwrap();
        let id = uuid::Uuid::new_v4();
        journal
            .append(&JobJournalEvent::Submitted {
                job_id: id,
                cargo_args: vec!["test".into()],
                executable: None,
                working_dir: None,
            })
            .unwrap();
        journal
            .append(&JobJournalEvent::Cancelled { job_id: id })
            .unwrap();
        let before = std::fs::metadata(&path).unwrap().len();
        journal.compact().unwrap();
        let after = std::fs::metadata(&path).unwrap().len();
        assert!(after <= before);
        let replay = journal.replay().unwrap();
        assert!(matches!(replay.jobs[0].status, JobStatus::Cancelled));
    }
}
