//! Workspace manager: materialize job workspaces from CAS blobs and release
//! their references on completion.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gitforge_common::{Error, ErrorKind, Result};
use tokio::io::AsyncWriteExt;

use crate::store::{BlobHash, CasStore};

/// A materialized workspace directory.
#[derive(Debug)]
pub struct WorkspaceHandle {
    /// The root directory the blobs were materialized into.
    pub path: PathBuf,
    /// Blobs referenced by this workspace (to be unref'd on release).
    blobs: Vec<BlobHash>,
}

impl WorkspaceHandle {
    /// Hashes held by this workspace.
    pub fn blobs(&self) -> &[BlobHash] {
        &self.blobs
    }
}

/// Materializes workspaces from CAS content and tracks their blob refs.
#[derive(Clone)]
pub struct WorkspaceManager {
    store: CasStore,
    workspaces_root: PathBuf,
}

impl WorkspaceManager {
    pub fn new(store: CasStore, workspaces_root: impl Into<PathBuf>) -> Self {
        Self {
            store,
            workspaces_root: workspaces_root.into(),
        }
    }

    /// Materialize a workspace: `blobs` maps relative paths inside the
    /// workspace to CAS hashes. All content is fetched first; only then are
    /// references taken, so a missing blob cannot leave orphaned refs.
    pub async fn materialize(
        &self,
        workspace_id: &str,
        blobs: &HashMap<String, BlobHash>,
    ) -> Result<WorkspaceHandle> {
        for rel in blobs.keys() {
            validate_rel_path(rel)?;
        }
        // Phase 1: fetch all content (no refs taken yet).
        let mut content = Vec::with_capacity(blobs.len());
        for (rel, hash) in blobs {
            let bytes = self.store.get(hash).await.map_err(|e| {
                Error::with_source(ErrorKind::Storage, format!("ws: missing blob for {rel}"), e)
            })?;
            content.push((rel.clone(), hash.clone(), bytes));
        }
        // Phase 2: take refs and write files.
        tokio::fs::create_dir_all(&self.workspaces_root)
            .await
            .map_err(|e| Error::with_source(ErrorKind::Storage, "ws: create root", e))?;
        let path = self.workspaces_root.join(sanitize_id(workspace_id));
        if tokio::fs::try_exists(&path)
            .await
            .map_err(|e| Error::with_source(ErrorKind::Storage, "ws: stat", e))?
        {
            return Err(Error::already_exists("workspace", workspace_id));
        }
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|e| Error::with_source(ErrorKind::Storage, "ws: create dir", e))?;

        let mut held = Vec::with_capacity(content.len());
        let mut write_error = None;
        for (rel, hash, bytes) in content {
            if write_error.is_none() {
                if let Err(e) = self.store.r#ref(&hash).await {
                    write_error = Some(e);
                    continue;
                }
                held.push(hash.clone());
                let target = path.join(&rel);
                if let Some(parent) = target.parent() {
                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                        write_error = Some(Error::with_source(ErrorKind::Storage, "ws: mkdir", e));
                        continue;
                    }
                }
                if let Err(e) = write_file(&target, &bytes).await {
                    write_error = Some(e);
                }
            }
        }
        if let Some(e) = write_error {
            return Err(e);
        }
        Ok(WorkspaceHandle { path, blobs: held })
    }

    /// Release a workspace: remove its directory and unref every blob it
    /// held. `now` is unix seconds supplied by the caller — the same
    /// injected-clock rule the store follows.
    pub async fn release(&self, handle: WorkspaceHandle, now_unix: i64) -> Result<()> {
        let result = match tokio::fs::remove_dir_all(&handle.path).await {
            Ok(()) => Ok(()),
            // Tolerate an already-removed workspace: refs still get released.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::with_source(ErrorKind::Storage, "ws: remove", e)),
        };
        for hash in &handle.blobs {
            self.store.unref(hash, now_unix).await?;
        }
        result
    }
}

async fn write_file(target: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = tokio::fs::File::create(target)
        .await
        .map_err(|e| Error::with_source(ErrorKind::Storage, "ws: create file", e))?;
    f.write_all(bytes)
        .await
        .map_err(|e| Error::with_source(ErrorKind::Storage, "ws: write file", e))?;
    f.flush()
        .await
        .map_err(|e| Error::with_source(ErrorKind::Storage, "ws: flush file", e))
}

fn validate_rel_path(rel: &str) -> Result<()> {
    let p = Path::new(rel);
    if p.is_absolute() || rel.split('/').any(|c| c == "..") || rel.contains('\\') || rel.is_empty()
    {
        return Err(Error::invalid_input(format!(
            "ws: unsafe blob path {rel:?}"
        )));
    }
    Ok(())
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
