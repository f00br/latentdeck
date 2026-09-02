use std::collections::BTreeMap;
use std::fs;

use latentdeck_control::{MetricsSnapshot, v2::MetricsSnapshot as Protocol2MetricsSnapshot};
use latentdeck_core::realtime_diagnostics::{
    DiagnosticBundleInput, DiagnosticCodecIdentity, DiagnosticCollectionLimits,
    DiagnosticEventLevel, DiagnosticEventRecord, DiagnosticEventSource, DiagnosticGpuIdentity,
    DiagnosticLogSource, DiagnosticProduct, DiagnosticProductIdentity,
    GenericDeckDiagnosticSession, InactiveApplicationDiagnosticSession, PlayerDiagnosticSession,
    PresentationDiagnosticCounters, Protocol2CodecIdentity, Protocol2ComputeDevice,
    Protocol2DeckSessionIdentity, Protocol2ExternalAssetIdentity, RealtimeDiagnosticError,
    RealtimeDiagnosticSession, RealtimeDiagnosticSnapshot, RealtimeSessionMetrics, SanitizedToken,
    Sha256Token, StableErrorRecord, StableErrorSource, TimingDistribution,
    WorkerDiagnosticCounters, collect_diagnostic_events, write_diagnostic_bundle_atomic,
};
use serde_json::Value;
use tempfile::TempDir;
use uuid::Uuid;

const TIMESTAMP_MS: u64 = 1_800_000_000_000;

fn token(value: &str) -> SanitizedToken {
    SanitizedToken::parse(value).expect("safe synthetic token")
}

fn hash(label: &[u8]) -> Sha256Token {
    Sha256Token::digest(label)
}

fn product(product: DiagnosticProduct) -> DiagnosticProductIdentity {
    DiagnosticProductIdentity::new(
        product,
        token("0.1.0"),
        token("latentdeck-core"),
        token("0.1.0"),
    )
}

fn gpu() -> DiagnosticGpuIdentity {
    DiagnosticGpuIdentity::new(
        SanitizedToken::from_hardware_label("NVIDIA GeForce RTX 4070").expect("adapter token"),
        SanitizedToken::from_hardware_label("NVIDIA 32.0.15.6094").expect("driver token"),
    )
}

fn codec() -> DiagnosticCodecIdentity {
    DiagnosticCodecIdentity::new(
        token("minimax_h3"),
        token("h3.0.1"),
        token("h3-playback"),
        token("0.1.0"),
        token("taehv-taeh3"),
        Some(hash(b"synthetic decoder identity")),
    )
}

fn worker_metrics() -> MetricsSnapshot {
    MetricsSnapshot {
        worker_uptime_ns: 9_000_000_000,
        decode_batches_total: 35,
        decoded_frames_total: 576,
        ring_backpressure_total: 2,
        presentation_skipped_total: 3,
        last_decode_duration_ns: 1_500_000,
        ring_write_sequence: 576,
        ring_read_sequence: 575,
        ring_occupancy: 1,
        gpu_allocated_bytes: Some(2_000_000_000),
        gpu_reserved_bytes: Some(3_000_000_000),
    }
}

fn session_metrics() -> RealtimeSessionMetrics {
    let frame_intervals =
        TimingDistribution::new(575, 39.0, 41.67, 43.0, 82.0).expect("frame distribution");
    let control_latency =
        TimingDistribution::new(12, 12.0, 55.0, 120.0, 150.0).expect("control distribution");
    let worker = WorkerDiagnosticCounters::from_metrics_snapshot(&worker_metrics())
        .expect("worker counters");
    let presentation = PresentationDiagnosticCounters::new(576, Some(3), Some(570))
        .expect("presentation counters");
    let errors = vec![
        StableErrorRecord::new(
            TIMESTAMP_MS - 100,
            StableErrorSource::Worker,
            token("worker.backpressure"),
        )
        .expect("stable error"),
    ];
    RealtimeSessionMetrics::new(
        24_000,
        24.0,
        23.99,
        frame_intervals,
        control_latency,
        worker,
        presentation,
        errors,
    )
    .expect("session metrics")
}

fn deck_snapshot() -> RealtimeDiagnosticSnapshot {
    let session = GenericDeckDiagnosticSession::new(
        token("org.example.operator"),
        vec![
            hash(b"cartridge a"),
            hash(b"cartridge b"),
            hash(b"cartridge c"),
            hash(b"cartridge d"),
        ],
        protocol2_session_metrics(),
        Protocol2DeckSessionIdentity::new(
            Uuid::parse_str("10000000-0000-4000-8000-000000000001").unwrap(),
            Uuid::parse_str("10000000-0000-4000-8000-000000000002").unwrap(),
            token("org.example.deck"),
            token("0.2.0"),
            token("0.2.0"),
        )
        .expect("P2 Deck identity"),
    )
    .expect("generic Deck session");
    RealtimeDiagnosticSnapshot::new(
        TIMESTAMP_MS,
        product(DiagnosticProduct::LatentDeck),
        gpu(),
        protocol2_codec_identity(),
        RealtimeDiagnosticSession::Deck(session),
    )
    .expect("Deck snapshot")
}

fn inactive_player_snapshot() -> RealtimeDiagnosticSnapshot {
    RealtimeDiagnosticSnapshot::new(
        TIMESTAMP_MS,
        product(DiagnosticProduct::LatentPlayer),
        gpu(),
        DiagnosticCodecIdentity::new(
            token("minimax_h3"),
            token("h3.0.1"),
            token("missing"),
            token("missing"),
            token("missing"),
            None,
        ),
        RealtimeDiagnosticSession::NoActiveSession(InactiveApplicationDiagnosticSession::new()),
    )
    .expect("inactive Player snapshot")
}

#[test]
fn bundle_contains_exact_path_free_realtime_layout() {
    let temporary = TempDir::new().expect("temporary directory");
    let destination = temporary.path().join("support.zip");
    let snapshot = deck_snapshot();
    let events = vec![
        DiagnosticEventRecord::new(
            TIMESTAMP_MS - 50,
            DiagnosticEventSource::Deck,
            DiagnosticEventLevel::Warn,
            token("worker.backpressure"),
            Some(token("ring_full")),
        )
        .expect("event"),
    ];
    let receipt = write_diagnostic_bundle_atomic(
        &destination,
        DiagnosticBundleInput::new(&snapshot, &events),
    )
    .expect("bundle");
    assert_eq!(receipt.event_count, 1);
    assert_eq!(receipt.schema_version, 1);
    assert_eq!(
        receipt.archive_bytes,
        fs::metadata(&destination).unwrap().len()
    );

    let archive = fs::read(&destination).expect("archive bytes");
    let parsed = parse_stored_zip(&archive);
    assert_eq!(
        parsed.keys().map(String::as_str).collect::<Vec<_>>(),
        ["events.jsonl", "manifest.json", "realtime.json"]
    );

    let manifest: Value = serde_json::from_slice(&parsed["manifest.json"]).unwrap();
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(
        manifest["bundle_format"],
        "latentdeck.realtime-diagnostic-bundle"
    );
    assert_eq!(manifest["product"], "latent_deck");
    assert_eq!(manifest["product_version"], "0.1.0");
    assert_eq!(manifest["app_version"], "0.1.0");
    assert_eq!(manifest["runtime"], "latentdeck-core");
    assert_eq!(manifest["runtime_version"], "0.1.0");
    assert_eq!(manifest["accepted_event_count"], 1);
    assert_eq!(
        manifest["entries"],
        serde_json::json!(["manifest.json", "events.jsonl", "realtime.json"])
    );
    assert!(
        manifest["privacy_exclusions"]
            .as_array()
            .unwrap()
            .contains(&Value::String("absolute_paths".to_owned()))
    );

    let realtime: Value = serde_json::from_slice(&parsed["realtime.json"]).unwrap();
    assert_realtime_payload(&realtime);

    let event_lines = std::str::from_utf8(&parsed["events.jsonl"])
        .unwrap()
        .lines()
        .collect::<Vec<_>>();
    assert_eq!(event_lines.len(), 1);
    let event: Value = serde_json::from_str(event_lines[0]).unwrap();
    assert_eq!(event["source"], "deck");
    assert_eq!(event["event"], "worker.backpressure");
    assert_eq!(event["code"], "ring_full");

    let rendered = String::from_utf8_lossy(&archive);
    assert!(!rendered.contains("C:\\"));
    assert!(!rendered.contains("W:\\"));
    assert!(!rendered.to_ascii_lowercase().contains("password="));
    assert_eq!(
        fs::read_dir(temporary.path()).unwrap().count(),
        1,
        "temporary archive must be removed"
    );
}

#[test]
fn protocol2_identity_is_exact_deterministic_and_path_free() {
    let protocol2_codec = protocol2_codec_identity();
    let metrics = protocol2_session_metrics();
    let worker_session = Uuid::parse_str("20000000-0000-4000-8000-000000000001").unwrap();
    let deck_session = Uuid::parse_str("20000000-0000-4000-8000-000000000002").unwrap();
    let session = GenericDeckDiagnosticSession::new(
        token("org.example.operator"),
        vec![hash(b"source a"), hash(b"source b")],
        metrics,
        Protocol2DeckSessionIdentity::new(
            worker_session,
            deck_session,
            token("org.example.deck"),
            token("0.2.0"),
            token("0.2.0"),
        )
        .expect("P2 Deck session"),
    )
    .expect("generic P2 session");
    let snapshot = RealtimeDiagnosticSnapshot::new(
        TIMESTAMP_MS,
        product(DiagnosticProduct::LatentDeck),
        gpu(),
        protocol2_codec,
        RealtimeDiagnosticSession::Deck(session),
    )
    .expect("P2 snapshot");
    let value = serde_json::to_value(snapshot).expect("serialize P2 snapshot");

    assert_eq!(value["codec"]["codec_family"], "synthetic_codec");
    assert_eq!(value["codec"]["profile"], "latent_signal");
    assert_eq!(value["codec"]["codec_pack"], "org.example.codec");
    assert_eq!(value["codec"]["codec_pack_version"], "2.4.1");
    assert!(value["codec"].get("decoder").is_none());
    assert!(value["codec"].get("decoder_sha256").is_none());
    assert_eq!(value["codec"]["protocol2"]["profile_version"], "7.3.0");
    assert_eq!(
        value["codec"]["protocol2"]["adapter"],
        "org.example.adapter"
    );
    assert_eq!(value["codec"]["protocol2"]["adapter_version"], "0.2.0");
    assert_eq!(value["codec"]["protocol2"]["compute_device"], "cpu");
    assert_eq!(value["codec"]["protocol2"]["device_ordinal"], 0);
    assert_eq!(
        value["codec"]["protocol2"]["external_assets"][0]["asset_id"],
        "asset-a"
    );
    assert_eq!(
        value["codec"]["protocol2"]["external_assets"][1]["asset_id"],
        "asset-z"
    );
    assert_eq!(
        value["deck"]["protocol2"]["worker_session_id"],
        worker_session.to_string()
    );
    assert_eq!(
        value["deck"]["protocol2"]["deck_session_id"],
        deck_session.to_string()
    );
    assert_eq!(
        value["deck"]["metrics"]["worker"]["protocol2"]["deck_process_total"],
        7
    );
    assert!(
        value["deck"]["metrics"]["worker"]
            .get("ring_occupancy")
            .is_none()
    );

    let encoded = serde_json::to_string(&value).unwrap();
    assert!(!encoded.contains("C:\\"));
    assert!(!encoded.contains("/Users/"));
    assert!(!encoded.contains("decoder"));
    let legacy = serde_json::to_value(codec()).expect("serialize P1 codec identity");
    assert_eq!(legacy["decoder"], "taehv-taeh3");
    assert!(legacy.get("protocol2").is_none());
}

#[test]
fn generic_protocol2_deck_snapshot_keeps_exact_package_and_source_identities() {
    let worker_session = Uuid::parse_str("30000000-0000-4000-8000-000000000001").unwrap();
    let deck_session = Uuid::parse_str("30000000-0000-4000-8000-000000000002").unwrap();
    let session = GenericDeckDiagnosticSession::new(
        token("org.example.operator"),
        vec![hash(b"source a"), hash(b"source b"), hash(b"source c")],
        protocol2_session_metrics(),
        Protocol2DeckSessionIdentity::new(
            worker_session,
            deck_session,
            token("org.example.deck"),
            token("0.2.0"),
            token("0.2.0"),
        )
        .expect("generic P2 identity"),
    )
    .expect("generic Deck diagnostics");
    let snapshot = RealtimeDiagnosticSnapshot::new(
        TIMESTAMP_MS,
        product(DiagnosticProduct::LatentDeck),
        gpu(),
        protocol2_codec_identity(),
        RealtimeDiagnosticSession::Deck(session),
    )
    .expect("generic snapshot");
    let value = serde_json::to_value(snapshot).expect("serialize generic snapshot");

    assert_eq!(value["deck"]["operator"], "org.example.operator");
    assert_eq!(
        value["deck"]["cartridge_sha256"].as_array().unwrap().len(),
        3
    );
    assert_eq!(
        value["deck"]["protocol2"]["deck_package"],
        "org.example.deck"
    );
    assert_eq!(
        value["deck"]["protocol2"]["worker_session_id"],
        worker_session.to_string()
    );
    assert!(value.get("player").is_none());
}

fn protocol2_codec_identity() -> DiagnosticCodecIdentity {
    let assets = vec![
        Protocol2ExternalAssetIdentity::new(token("asset-z"), hash(b"asset z"), 22)
            .expect("asset z"),
        Protocol2ExternalAssetIdentity::new(token("asset-a"), hash(b"asset a"), 11)
            .expect("asset a"),
    ];
    DiagnosticCodecIdentity::new_protocol2(
        token("synthetic_codec"),
        token("latent_signal"),
        token("org.example.codec"),
        token("2.4.1"),
        Protocol2CodecIdentity::new(
            token("7.3.0"),
            token("org.example.adapter"),
            token("0.2.0"),
            Protocol2ComputeDevice::Cpu,
            0,
            assets,
        )
        .expect("P2 codec identity"),
    )
}

fn protocol2_session_metrics() -> RealtimeSessionMetrics {
    let worker =
        WorkerDiagnosticCounters::from_protocol2_metrics_snapshot(&Protocol2MetricsSnapshot {
            worker_uptime_ns: 10,
            commands_total: 9,
            commands_failed_total: 1,
            player_steps_total: 0,
            deck_process_total: 7,
            capture_slots_total: 3,
            decoded_frames_total: 24,
        })
        .expect("P2 worker counters");
    RealtimeSessionMetrics::new(
        1_000,
        24.0,
        0.0,
        TimingDistribution::new(0, 0.0, 0.0, 0.0, 0.0).unwrap(),
        TimingDistribution::new(0, 0.0, 0.0, 0.0, 0.0).unwrap(),
        worker,
        PresentationDiagnosticCounters::new(0, None, Some(0)).unwrap(),
        Vec::new(),
    )
    .expect("P2 session metrics")
}

#[test]
fn protocol2_identity_rejects_unsafe_or_ambiguous_values() {
    assert!(SanitizedToken::parse("C:\\private\\asset").is_err());
    let duplicated = vec![
        Protocol2ExternalAssetIdentity::new(token("same"), hash(b"one"), 1).unwrap(),
        Protocol2ExternalAssetIdentity::new(token("same"), hash(b"two"), 2).unwrap(),
    ];
    assert!(
        Protocol2CodecIdentity::new(
            token("1.0.0"),
            token("org.example.adapter"),
            token("1.0.0"),
            Protocol2ComputeDevice::Cuda,
            1,
            duplicated,
        )
        .is_err()
    );
    assert!(
        Protocol2DeckSessionIdentity::new(
            Uuid::nil(),
            Uuid::new_v4(),
            token("org.example.deck"),
            token("1.0.0"),
            token("1.0.0"),
        )
        .is_err()
    );
}

fn assert_realtime_payload(realtime: &Value) {
    assert_eq!(realtime["product"]["product"], "latent_deck");
    assert_eq!(realtime["gpu"]["adapter"], "NVIDIA-GeForce-RTX-4070");
    assert_eq!(realtime["gpu"]["driver"], "NVIDIA-32.0.15.6094");
    assert_eq!(realtime["codec"]["codec_family"], "synthetic_codec");
    assert_eq!(
        realtime["codec"]["protocol2"]["adapter"],
        "org.example.adapter"
    );
    assert_eq!(realtime["deck"]["operator"], "org.example.operator");
    assert_eq!(
        realtime["deck"]["cartridge_sha256"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        realtime["deck"]["protocol2"]["deck_package"],
        "org.example.deck"
    );
    let metrics = &realtime["deck"]["metrics"];
    assert_eq!(metrics["target_fps"], 24.0);
    assert_eq!(metrics["measured_fps"], 0.0);
    assert_eq!(metrics["worker"]["protocol2"]["commands_total"], 9);
    assert_eq!(metrics["worker"]["protocol2"]["deck_process_total"], 7);
    assert_eq!(metrics["worker"]["protocol2"]["capture_slots_total"], 3);
    assert_eq!(metrics["presentation"]["frames_presented"], 0);
}

#[test]
fn identical_input_produces_identical_archive_bytes() {
    let temporary = TempDir::new().unwrap();
    let snapshot = deck_snapshot();
    let events = [DiagnosticEventRecord::new(
        TIMESTAMP_MS,
        DiagnosticEventSource::Deck,
        DiagnosticEventLevel::Info,
        token("diagnostics.captured"),
        None,
    )
    .unwrap()];
    let first = temporary.path().join("first.zip");
    let second = temporary.path().join("second.zip");
    let input = DiagnosticBundleInput::new(&snapshot, &events);
    write_diagnostic_bundle_atomic(&first, input).unwrap();
    write_diagnostic_bundle_atomic(&second, input).unwrap();
    assert_eq!(fs::read(first).unwrap(), fs::read(second).unwrap());
}

#[test]
fn inactive_startup_and_active_player_sessions_are_truthful() {
    let temporary = TempDir::new().unwrap();
    let inactive = inactive_player_snapshot();
    let inactive_path = temporary.path().join("inactive.zip");
    write_diagnostic_bundle_atomic(&inactive_path, DiagnosticBundleInput::new(&inactive, &[]))
        .expect("inactive bundle");
    let parsed = parse_stored_zip(&fs::read(&inactive_path).unwrap());
    let realtime: Value = serde_json::from_slice(&parsed["realtime.json"]).unwrap();
    assert_eq!(realtime["inactive_application"]["no_active_session"], true);
    assert!(realtime.get("deck").is_none());
    assert!(realtime.get("player").is_none());

    let player = PlayerDiagnosticSession::new(hash(b"player cartridge"), session_metrics());
    let active = RealtimeDiagnosticSnapshot::new(
        TIMESTAMP_MS,
        product(DiagnosticProduct::LatentPlayer),
        gpu(),
        codec(),
        RealtimeDiagnosticSession::Player(player.clone()),
    )
    .expect("active Player snapshot");
    let active_path = temporary.path().join("active.zip");
    write_diagnostic_bundle_atomic(&active_path, DiagnosticBundleInput::new(&active, &[]))
        .expect("active bundle");
    let parsed = parse_stored_zip(&fs::read(&active_path).unwrap());
    let realtime: Value = serde_json::from_slice(&parsed["realtime.json"]).unwrap();
    assert!(realtime.get("inactive_application").is_none());
    assert_eq!(
        realtime["player"]["metrics"]["worker"]["decoded_frames_total"],
        576
    );

    let mismatch = RealtimeDiagnosticSnapshot::new(
        TIMESTAMP_MS,
        product(DiagnosticProduct::LatentDeck),
        gpu(),
        codec(),
        RealtimeDiagnosticSession::Player(player),
    )
    .expect_err("Deck cannot contain Player runtime");
    assert!(matches!(
        mismatch,
        RealtimeDiagnosticError::ProductSessionMismatch
    ));
    let conflict = inactive
        .with_session(RealtimeDiagnosticSession::Player(
            PlayerDiagnosticSession::new(hash(b"late player"), session_metrics()),
        ))
        .expect_err("inactive state cannot gain realtime metrics");
    assert!(matches!(
        conflict,
        RealtimeDiagnosticError::ConflictingSessionState
    ));
}

#[test]
fn installed_collector_is_bounded_and_reserializes_only_allowlisted_records() {
    let temporary = TempDir::new().unwrap();
    let deck = temporary.path().join("deck");
    let player = temporary.path().join("player");
    let worker = temporary.path().join("worker");
    fs::create_dir_all(deck.join("nested")).unwrap();
    fs::create_dir_all(&player).unwrap();
    fs::create_dir_all(&worker).unwrap();

    fs::write(
        deck.join("latentdeck-1.jsonl"),
        concat!(
            "{\"schema_version\":1,\"timestamp_unix_ms\":1800000000000,\"level\":\"info\",\"event\":\"app.started\"}\n",
            "{\"schema_version\":1,\"timestamp_unix_ms\":1800000000001,\"level\":\"error\",\"event\":\"app.failed\",\"code\":\"C:\\\\Users\\\\private\",\"message\":\"password=private\"}\n",
            "not-json\n"
        ),
    )
    .unwrap();
    fs::write(
        player.join("latentplayer-1.jsonl"),
        "{\"schema_version\":1,\"timestamp_unix_ms\":1800000000002,\"level\":\"warn\",\"event\":\"codec.missing\",\"code\":\"codec_missing\"}\n",
    )
    .unwrap();
    fs::write(
        worker.join("worker-1.jsonl"),
        concat!(
            "{\"schema_version\":1,\"timestamp_ns\":1800000000003000000,\"event\":\"decode_failed\",\"error_type\":\"RuntimeError\",\"pid\":1234,\"detail\":\"W:\\\\private\"}\n",
            "{\"schema_version\":1,\"timestamp_ns\":1800000000004000000,\"event\":\"decode_failed\",\"detail\":\"private path\"}\n"
        ),
    )
    .unwrap();
    fs::write(deck.join("latentdeck-oversized.jsonl"), vec![b'x'; 513]).unwrap();
    fs::write(
        deck.join("nested").join("latentdeck-nested.jsonl"),
        "{\"schema_version\":1}",
    )
    .unwrap();
    fs::write(deck.join("unrelated.jsonl"), "not considered").unwrap();

    let sources = [
        DiagnosticLogSource::new(DiagnosticEventSource::Deck, &deck),
        DiagnosticLogSource::new(DiagnosticEventSource::Player, &player),
        DiagnosticLogSource::new(DiagnosticEventSource::Worker, &worker),
    ];
    let limits = DiagnosticCollectionLimits::new(8, 512, 2_048, 8).unwrap();
    let collection = collect_diagnostic_events(&sources, limits).expect("collection");
    assert_eq!(collection.events().len(), 5);
    assert_eq!(collection.report().accepted_event_count, 5);
    assert_eq!(collection.report().dropped_record_count, 1);
    assert_eq!(collection.report().processed_file_count, 3);
    assert!(collection.report().skipped_file_count >= 1);
    assert_eq!(
        collection.report().included_sources,
        [
            DiagnosticEventSource::Deck,
            DiagnosticEventSource::Player,
            DiagnosticEventSource::Worker
        ]
    );

    let output = temporary.path().join("collected.zip");
    let snapshot = inactive_player_snapshot();
    write_diagnostic_bundle_atomic(
        &output,
        DiagnosticBundleInput::from_collection(&snapshot, &collection),
    )
    .expect("collected bundle");
    let parsed = parse_stored_zip(&fs::read(&output).unwrap());
    let manifest: Value = serde_json::from_slice(&parsed["manifest.json"]).unwrap();
    assert_eq!(manifest["accepted_event_count"], 5);
    assert_eq!(manifest["dropped_record_count"], 1);
    assert_eq!(manifest["processed_file_count"], 3);
    assert_eq!(
        manifest["included_sources"],
        serde_json::json!(["deck", "player", "worker"])
    );
    let events = std::str::from_utf8(&parsed["events.jsonl"]).unwrap();
    assert_eq!(events.lines().count(), 5);
    assert!(!events.contains("Users"));
    assert!(!events.contains("private path"));
    assert!(!events.contains("password="));
    assert!(!events.contains("W:\\"));
    assert!(!events.contains("\"message\""));
    assert!(!events.contains("\"detail\""));
    assert!(!events.contains("\"pid\""));
    assert!(events.contains("RuntimeError"));

    let tight = collect_diagnostic_events(
        &sources,
        DiagnosticCollectionLimits::new(8, 512, 2_048, 2).unwrap(),
    )
    .expect("tight collection");
    assert_eq!(tight.events().len(), 2);
    assert_eq!(tight.report().accepted_event_count, 2);
    assert!(tight.report().dropped_record_count >= 4);
}

#[test]
fn validators_reject_paths_secrets_nonfinite_values_and_oversized_state() {
    assert_eq!(
        SanitizedToken::from_hardware_label("NVIDIA GeForce RTX 4070 (AD104)")
            .unwrap()
            .as_str(),
        "NVIDIA-GeForce-RTX-4070-AD104"
    );
    for unsafe_value in [
        "C:\\Users\\private\\gpu",
        "https://driver.invalid",
        "token=private",
        "$ENV_VALUE",
        "line\nbreak",
    ] {
        assert!(SanitizedToken::from_hardware_label(unsafe_value).is_err());
    }
    assert!(SanitizedToken::parse("../private").is_err());
    assert_eq!(
        Sha256Token::parse(&"A".repeat(64)).unwrap().as_str(),
        "a".repeat(64)
    );
    assert!(Sha256Token::parse("not-a-hash").is_err());
    assert!(TimingDistribution::new(1, 1.0, f64::NAN, 2.0, 3.0).is_err());
    assert!(TimingDistribution::new(0, 0.0, 0.0, 0.0, 1.0).is_err());

    let mut invalid_worker = worker_metrics();
    invalid_worker.ring_occupancy = 4_097;
    assert!(WorkerDiagnosticCounters::from_metrics_snapshot(&invalid_worker).is_err());
    assert!(PresentationDiagnosticCounters::new(u64::MAX, None, None).is_err());
    assert!(DiagnosticCollectionLimits::new(0, 1, 1, 1).is_err());
    let sanitized = DiagnosticEventRecord::from_application_json_line(
        DiagnosticEventSource::Deck,
        br#"{"schema_version":1,"timestamp_unix_ms":1800000000000,"level":"info","event":"app.started","message":"arbitrary"}"#,
    )
    .expect("unknown fields are stripped");
    let sanitized = serde_json::to_string(&sanitized).unwrap();
    assert!(!sanitized.contains("message"));
    assert!(!sanitized.contains("arbitrary"));
    assert!(
        DiagnosticEventRecord::new(
            0,
            DiagnosticEventSource::Deck,
            DiagnosticEventLevel::Info,
            token("app.started"),
            None,
        )
        .is_err()
    );

    let error = StableErrorRecord::new(
        TIMESTAMP_MS,
        StableErrorSource::Control,
        token("control.invalid"),
    )
    .unwrap();
    let too_many = vec![error; 257];
    assert!(
        RealtimeSessionMetrics::new(
            1,
            24.0,
            0.0,
            TimingDistribution::new(0, 0.0, 0.0, 0.0, 0.0).unwrap(),
            TimingDistribution::new(0, 0.0, 0.0, 0.0, 0.0).unwrap(),
            WorkerDiagnosticCounters::from_metrics_snapshot(&worker_metrics()).unwrap(),
            PresentationDiagnosticCounters::new(0, None, None).unwrap(),
            too_many,
        )
        .is_err()
    );
}

#[test]
fn no_clobber_failure_preserves_destination_and_cleans_partial() {
    let temporary = TempDir::new().unwrap();
    let destination = temporary.path().join("support.zip");
    fs::write(&destination, b"owner data").unwrap();
    let snapshot = deck_snapshot();
    let error =
        write_diagnostic_bundle_atomic(&destination, DiagnosticBundleInput::new(&snapshot, &[]))
            .expect_err("must not overwrite");
    assert!(matches!(error, RealtimeDiagnosticError::OutputExists));
    assert_eq!(fs::read(&destination).unwrap(), b"owner data");
    let names = fs::read_dir(temporary.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, ["support.zip"]);
}

fn parse_stored_zip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut cursor = 0_usize;
    let mut entries = BTreeMap::new();
    let mut local_offsets = BTreeMap::new();
    while read_u32(bytes, cursor) == 0x0403_4b50 {
        let local_offset = cursor;
        assert_eq!(read_u16(bytes, cursor + 8), 0, "STORE method");
        let expected_crc = read_u32(bytes, cursor + 14);
        let compressed_size = usize::try_from(read_u32(bytes, cursor + 18)).unwrap();
        let uncompressed_size = usize::try_from(read_u32(bytes, cursor + 22)).unwrap();
        assert_eq!(compressed_size, uncompressed_size);
        let name_length = usize::from(read_u16(bytes, cursor + 26));
        let extra_length = usize::from(read_u16(bytes, cursor + 28));
        let name_start = cursor + 30;
        let name_end = name_start + name_length;
        let name = std::str::from_utf8(&bytes[name_start..name_end])
            .unwrap()
            .to_owned();
        let data_start = name_end + extra_length;
        let data_end = data_start + compressed_size;
        let data = bytes[data_start..data_end].to_vec();
        assert_eq!(crc32fast::hash(&data), expected_crc);
        assert!(entries.insert(name.clone(), data).is_none());
        local_offsets.insert(name, u32::try_from(local_offset).unwrap());
        cursor = data_end;
    }
    let central_offset = cursor;
    let mut central_names = Vec::new();
    while read_u32(bytes, cursor) == 0x0201_4b50 {
        assert_eq!(read_u16(bytes, cursor + 10), 0, "STORE method");
        let name_length = usize::from(read_u16(bytes, cursor + 28));
        let extra_length = usize::from(read_u16(bytes, cursor + 30));
        let comment_length = usize::from(read_u16(bytes, cursor + 32));
        let local_offset = read_u32(bytes, cursor + 42);
        let name_start = cursor + 46;
        let name_end = name_start + name_length;
        let name = std::str::from_utf8(&bytes[name_start..name_end])
            .unwrap()
            .to_owned();
        assert_eq!(local_offsets[&name], local_offset);
        central_names.push(name);
        cursor = name_end + extra_length + comment_length;
    }
    assert_eq!(
        central_names,
        ["manifest.json", "events.jsonl", "realtime.json"]
    );
    assert_eq!(read_u32(bytes, cursor), 0x0605_4b50);
    assert_eq!(read_u16(bytes, cursor + 8), 3);
    assert_eq!(read_u16(bytes, cursor + 10), 3);
    assert_eq!(
        usize::try_from(read_u32(bytes, cursor + 16)).unwrap(),
        central_offset
    );
    assert_eq!(
        usize::try_from(read_u32(bytes, cursor + 12)).unwrap(),
        cursor - central_offset
    );
    assert_eq!(read_u16(bytes, cursor + 20), 0);
    assert_eq!(cursor + 22, bytes.len());
    entries
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
