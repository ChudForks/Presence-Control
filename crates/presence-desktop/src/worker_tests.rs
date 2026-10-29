use super::{worker, Command, Update};
use presence_core::{model::UsageData, Backend};
use serde_json::json;
use std::{
    sync::{mpsc, Arc},
    time::Duration,
};
use tokio::sync::mpsc::unbounded_channel;

async fn shutdown_result(receiver: &mpsc::Receiver<Update>) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match receiver.try_recv() {
                Ok(Update::Shutdown(result)) => return result,
                Ok(_) => {}
                Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                Err(mpsc::TryRecvError::Disconnected) => {
                    panic!("worker exited before shutdown result")
                }
            }
        }
    })
    .await
    .expect("shutdown result timed out")
}

/// Exercise the real worker/backend/store boundary: failed checkpointing must
/// leave the command receiver alive so the UI's offered retry can actually save.
#[tokio::test(flavor = "current_thread")]
async fn failed_shutdown_preserves_worker_and_retry_persists_pending_history_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let session = json!({
        "id": "finished-session", "ruleId": "editor", "name": "Editor",
        "process": "Code.exe", "startedAt": 1_000, "endedAt": 11_000,
        "durationMs": 10_000,
    });
    // A completed archive write plus stale recent history represents an
    // interrupted prior save. Opening the backend schedules its safe cleanup.
    std::fs::write(
        root.path().join("usage.json"),
        serde_json::to_vec(&json!({
            "totals": { "editor": 10_000 }, "sessions": [session.clone()],
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.path().join("archive.json"),
        serde_json::to_vec(&json!({
            "entries": [{ "kind": "session", "entry": session, "archivedAt": 12_000 }],
        }))
        .unwrap(),
    )
    .unwrap();
    let (backend, warnings) = Backend::open(root.path(), Arc::new(|_| {})).await;
    assert!(warnings.is_empty());

    // Do this before yielding the current-thread runtime, so the first periodic
    // checkpoint cannot clean the pending history before the failure is installed.
    let usage_path = root.path().join("usage.json");
    std::fs::remove_file(&usage_path).unwrap();
    std::fs::create_dir(&usage_path).unwrap();
    let (updates, receiver) = mpsc::channel();
    let (sender, commands) = unbounded_channel();
    let task = tokio::spawn(worker(backend, commands, updates));
    sender.send(Command::Shutdown).unwrap();
    let error = shutdown_result(&receiver).await.unwrap_err();
    assert!(error.contains("Could not replace local data"), "{error}");
    assert!(
        !task.is_finished(),
        "failed checkpoint must retain the worker"
    );

    // Removing only this empty test-owned blocker makes the retry write succeed.
    std::fs::remove_dir(&usage_path).unwrap();
    sender.send(Command::Shutdown).unwrap();
    shutdown_result(&receiver).await.unwrap();
    task.await.unwrap();
    let saved: UsageData = serde_json::from_slice(&std::fs::read(&usage_path).unwrap()).unwrap();
    assert!(
        saved.sessions.is_empty(),
        "archived session cleanup must persist"
    );
    assert_eq!(
        saved.totals["editor"], 10_000,
        "session archive preserves totals"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn failed_linked_archive_shutdown_can_retry_without_losing_the_saved_total() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("usage.json"),
        serde_json::to_vec(&json!({
            "totals":{"editor":20000}, "sessions":[], "totalIds":{"editor":"total"}
        }))
        .unwrap(),
    )
    .unwrap();
    let archive_path = root.path().join("archive.json");
    std::fs::write(&archive_path, serde_json::to_vec(&json!({"entries":[{
        "kind":"application", "archivedAt":1000, "tracksApplicationTime":true,
        "entry":{"id":"total","ruleId":"editor","name":"Editor","process":"Code.exe","durationMs":10000}
    }]})).unwrap()).unwrap();
    let (backend, warnings) = Backend::open(root.path(), Arc::new(|_| {})).await;
    assert!(warnings.is_empty());
    // Block the pending archive repair before allowing any scheduled work.
    std::fs::remove_file(&archive_path).unwrap();
    std::fs::create_dir(&archive_path).unwrap();
    let (updates, receiver) = mpsc::channel();
    let (sender, commands) = unbounded_channel();
    let task = tokio::spawn(worker(backend, commands, updates));
    sender.send(Command::Shutdown).unwrap();
    assert!(shutdown_result(&receiver)
        .await
        .unwrap_err()
        .contains("Could not replace local data"));
    assert!(
        !task.is_finished(),
        "archive failures must retain the worker for retry"
    );
    std::fs::remove_dir(&archive_path).unwrap();
    sender.send(Command::Shutdown).unwrap();
    shutdown_result(&receiver).await.unwrap();
    task.await.unwrap();
    let archive: presence_core::model::ArchiveData =
        serde_json::from_slice(&std::fs::read(&archive_path).unwrap()).unwrap();
    assert_eq!(archive.entries[0].entry["durationMs"], 20_000);
    assert!(archive.entries[0].tracks_application_time());
    let usage: UsageData =
        serde_json::from_slice(&std::fs::read(root.path().join("usage.json")).unwrap()).unwrap();
    assert_eq!(usage.totals["editor"], 20_000);
}

#[tokio::test(flavor = "current_thread")]
async fn failed_stop_save_still_reports_disabled_automatic_mode_to_the_ui() {
    let root = tempfile::tempdir().unwrap();
    let config_path = root.path().join("config.json");
    let config = presence_core::model::Config {
        automatic: true,
        ..Default::default()
    };
    std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let (backend, warnings) = Backend::open(root.path(), Arc::new(|_| {})).await;
    assert!(warnings.is_empty());
    std::fs::remove_file(&config_path).unwrap();
    std::fs::create_dir(&config_path).unwrap();
    let (updates, receiver) = mpsc::channel();
    let (sender, commands) = unbounded_channel();
    let task = tokio::spawn(worker(backend, commands, updates));
    sender.send(Command::Stop).unwrap();

    let mut saw_config = false;
    let mut saw_runtime = false;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match receiver.try_recv() {
                Ok(Update::Saved { config, selected }) => {
                    assert!(
                        !config.automatic,
                        "failed stop must not leave the toggle on"
                    );
                    assert!(selected.is_none());
                    saw_config = true;
                }
                Ok(Update::Runtime(runtime)) => {
                    assert!(runtime.mode.is_none());
                    assert!(runtime.active_profile_id.is_none());
                    saw_runtime = true;
                }
                Ok(Update::Finished(result)) => {
                    assert!(result.unwrap_err().contains("Could not replace local data"));
                    break;
                }
                Ok(_) => {}
                Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                Err(mpsc::TryRecvError::Disconnected) => panic!("worker stopped unexpectedly"),
            }
        }
    })
    .await
    .expect("stop result timed out");
    assert!(saw_config && saw_runtime);
    sender.send(Command::Shutdown).unwrap();
    shutdown_result(&receiver).await.unwrap();
    task.await.unwrap();
}
