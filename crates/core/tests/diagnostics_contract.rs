use std::fs;

use latentdeck_core::diagnostics::{LogLevel, StructuredLog};
use serde_json::Value;
use tempfile::tempdir;

#[test]
fn structured_log_is_path_free_bounded_and_whole_record_only() {
    let root = tempdir().expect("tempdir");
    let log = StructuredLog::open_with_limit(root.path(), "latentdeck", 420).expect("open log");

    log.record(LogLevel::Info, "app.started", None)
        .expect("startup record");
    log.record(
        LogLevel::Warn,
        "worker.failed",
        Some("worker.handshake_timeout"),
    )
    .expect("failure record");
    for _ in 0..32 {
        log.record(LogLevel::Info, "runtime.heartbeat", None)
            .expect("bounded record attempt");
    }

    let path = root.path().join(log.file_name());
    let bytes = fs::read(&path).expect("read log");
    assert!(bytes.len() <= 420);
    assert!(bytes.ends_with(b"\n"));
    let rendered = String::from_utf8(bytes).expect("utf8");
    assert!(!rendered.contains(&root.path().display().to_string()));
    let records: Vec<Value> = rendered
        .lines()
        .map(|line| serde_json::from_str(line).expect("whole json record"))
        .collect();
    assert!(records.len() >= 2);
    assert_eq!(records[0]["schema_version"], 1);
    assert_eq!(records[0]["event"], "app.started");
    assert_eq!(records[1]["code"], "worker.handshake_timeout");
}

#[test]
fn structured_log_rejects_unbounded_or_path_like_fields() {
    let root = tempdir().expect("tempdir");
    let log = StructuredLog::open_with_limit(root.path(), "latentplayer", 1024).expect("open log");

    assert!(log.record(LogLevel::Error, "bad event", None).is_err());
    assert!(
        log.record(LogLevel::Error, "app.failed", Some(r"C:\private\asset.lc"))
            .is_err()
    );
}

#[test]
fn structured_log_retention_is_product_scoped_and_bounded() {
    let root = tempdir().expect("tempdir");
    fs::write(root.path().join("unrelated.jsonl"), b"keep\n").expect("unrelated fixture");
    let player = StructuredLog::open(root.path(), "latentplayer").expect("player log");
    player
        .record(LogLevel::Info, "app.started", None)
        .expect("player record");
    let player_name = player.file_name().to_owned();
    drop(player);

    for _ in 0..20 {
        let deck = StructuredLog::open(root.path(), "latentdeck").expect("deck log");
        deck.record(LogLevel::Info, "app.started", None)
            .expect("deck record");
    }

    let deck_files = fs::read_dir(root.path())
        .expect("read log directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_name().to_str().is_some_and(|name| {
                name.starts_with("latentdeck-")
                    && std::path::Path::new(name)
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
            })
        })
        .count();
    assert_eq!(deck_files, 16);
    assert!(root.path().join(player_name).is_file());
    assert!(root.path().join("unrelated.jsonl").is_file());
}
