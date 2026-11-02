//! Integration test: full put -> materialize -> unref -> sweep cycle (#339).

use std::collections::HashMap;

use gitforge_cas::{CasStore, WorkspaceManager};

#[tokio::test]
async fn full_materialize_cycle() {
    let root = tempfile::tempdir().expect("tempdir");
    let ws_root = tempfile::tempdir().expect("ws tempdir");
    let store = CasStore::open(root.path().join("cas")).await.expect("open");

    let h1 = store.put_ref(b"pipeline script").await.expect("put_ref");
    let h2 = store.put_ref(b"config payload").await.expect("put_ref");

    let mgr = WorkspaceManager::new(store.clone(), ws_root.path().join("workspaces"));
    let mut blobs = HashMap::new();
    blobs.insert("scripts/run.sh".to_string(), h1.clone());
    blobs.insert("etc/config.toml".to_string(), h2.clone());
    blobs.insert("nested/deep/file.txt".to_string(), h1.clone());

    let handle = mgr
        .materialize("job-42", &blobs)
        .await
        .expect("materialize");
    assert_eq!(
        tokio::fs::read(handle.path.join("scripts/run.sh"))
            .await
            .expect("read"),
        b"pipeline script"
    );
    assert_eq!(
        tokio::fs::read(handle.path.join("nested/deep/file.txt"))
            .await
            .expect("read"),
        b"pipeline script"
    );
    // put_ref contributed 1 each; materialize adds one per entry
    // (h1 twice, h2 once).
    assert_eq!(store.refcount(&h1).await.expect("rc"), 3);
    assert_eq!(store.refcount(&h2).await.expect("rc"), 2);

    // Job completes: release the workspace.
    let now_unix = 1_800_000_000;
    mgr.release(handle, now_unix).await.expect("release");
    // The original put_ref refs remain (1 each); the workspace's refs
    // (h1 x2, h2 x1) were released.
    assert_eq!(store.refcount(&h1).await.expect("rc"), 1);
    assert_eq!(store.refcount(&h2).await.expect("rc"), 1);
    assert!(
        !tokio::fs::try_exists(ws_root.path().join("workspaces/job-42"))
            .await
            .expect("stat")
    );

    // Blob files still exist before GC.
    assert!(store.get(&h1).await.is_ok());

    // Both blobs still hold their original put_ref, so GC deletes
    // nothing even far past the grace period.
    let outcome = store
        .sweep(1_800_000_000 + 10 * 86_400, 86_400)
        .await
        .expect("sweep");
    assert_eq!(outcome.deleted, 0);
    assert_eq!(outcome.deferred, 0);
    assert!(store.get(&h1).await.is_ok());
    assert!(store.get(&h2).await.is_ok());

    // Consumer releases the last refs; GC then collects both.
    let late = 1_800_000_000 + 10 * 86_400;
    store.unref(&h1, late).await.expect("unref");
    store.unref(&h2, late).await.expect("unref");
    let outcome = store.sweep(late + 86_400 + 1, 86_400).await.expect("sweep");
    assert_eq!(outcome.deleted, 2);
    assert!(store.get(&h1).await.is_err());
    assert!(store.get(&h2).await.is_err());

    store.close().await;
}

#[tokio::test]
async fn materialize_rejects_path_escape() {
    let root = tempfile::tempdir().expect("tempdir");
    let ws_root = tempfile::tempdir().expect("ws tempdir");
    let store = CasStore::open(root.path().join("cas")).await.expect("open");
    let h = store.put_ref(b"x").await.expect("put_ref");
    let mgr = WorkspaceManager::new(store.clone(), ws_root.path());
    let mut blobs = HashMap::new();
    blobs.insert("../escape".to_string(), h.clone());
    let err = mgr
        .materialize("evil", &blobs)
        .await
        .expect_err("must reject");
    assert!(err.to_string().contains("unsafe blob path"));
    store.close().await;
}

#[tokio::test]
async fn materialize_missing_blob_takes_no_refs() {
    let root = tempfile::tempdir().expect("tempdir");
    let ws_root = tempfile::tempdir().expect("ws tempdir");
    let store = CasStore::open(root.path().join("cas")).await.expect("open");
    let ghost = gitforge_cas::store::hash_bytes(b"never stored");
    let mgr = WorkspaceManager::new(store.clone(), ws_root.path());
    let mut blobs = HashMap::new();
    blobs.insert("f".to_string(), ghost.clone());
    assert!(mgr.materialize("ws", &blobs).await.is_err());
    // No partial directory, no refs leaked.
    assert!(!tokio::fs::try_exists(ws_root.path().join("ws"))
        .await
        .expect("stat"));
    store.close().await;
}
