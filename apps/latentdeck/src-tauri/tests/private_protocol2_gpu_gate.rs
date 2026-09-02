//! Path-free acceptance contract for the opt-in private Protocol 2 GPU gate.
//!
//! This target never starts CUDA during ordinary workspace tests. The ignored
//! receipt validator is invoked only after an explicit private run and accepts
//! evidence, not machine-local paths or private payload bytes.

use std::{collections::BTreeSet, env, fs, path::Path};

use serde::Deserialize;
use serde_json::{Value, json};

const OPT_IN_ENV: &str = "LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE";
const RECEIPT_ENV: &str = "LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE_RECEIPT";
const CODEC_ID: &str = "org.latentdeck.h3";
const CODEC_VERSION: &str = "0.2.0";
const ADAPTER_ID: &str = "org.latentdeck.h3";
const ADAPTER_VERSION: &str = "0.2.0";
const D2_ID: &str = "org.latentdeck.deck.d2";
const Q4_ID: &str = "org.latentdeck.deck.q4";
const EXTERNAL_DECK_ID: &str = "dev.latentdeck.private.h3_probe";
const DECK_VERSION: &str = "0.2.0";
const PROFILE_FAMILY: &str = "minimax_h3";
const PROFILE_NAME: &str = "h3_av_latent";
const PROFILE_VERSION: &str = "0.1.0";
const TORCH_BUILD: &str = "2.13.0+cu130";
const STABILITY_SECONDS: u64 = 360;

const CAPABILITIES: [&str; 6] = [
    "player",
    "realtime",
    "resample",
    "snapshot_capture",
    "live_capture",
    "raw_import",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GateReceipt {
    schema_version: u8,
    evidence_kind: String,
    result: String,
    source_commit: String,
    git_dirty: bool,
    protocol: ProtocolEvidence,
    packages: PackageEvidence,
    profile: ProfileEvidence,
    inputs: InputEvidence,
    coverage: CoverageEvidence,
    safety: SafetyEvidence,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolEvidence {
    worker_protocol: u16,
    worker_module: String,
    codec_host_api: String,
    codec_manifest_version: String,
    adapter_entrypoint: String,
    capabilities: Vec<String>,
    p1_fallback_attempted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageEvidence {
    codec: ExactPackage,
    adapter: ExactPackage,
    decks: Vec<ExactPackage>,
    external_deck: ExactPackage,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ExactPackage {
    id: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileEvidence {
    codec_family: String,
    profile: String,
    profile_version: String,
    python: String,
    torch: String,
    device: String,
    device_ordinal: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)]
struct InputEvidence {
    codec_pack_sha256: String,
    decoder_sha256: String,
    source_archive_sha256: Vec<String>,
    external_deck_archive_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoverageEvidence {
    player: PlayerCoverage,
    d2: DeckCoverage,
    q4: DeckCoverage,
    external_deck: ExternalDeckCoverage,
    stability: Vec<StabilityCoverage>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlayerCoverage {
    opened: bool,
    decoded_frames: u64,
    reset_generation_before: u64,
    reset_generation_after: u64,
    reset_confirmed: bool,
    status_checked: bool,
    status_state: String,
    spout: SpoutCoverage,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeckCoverage {
    opened: bool,
    processed_frames: u64,
    reset_generation_before: u64,
    reset_generation_after: u64,
    reset_confirmed: bool,
    status_checked: bool,
    status_state: String,
    snapshot: CaptureCoverage,
    live_capture: CaptureCoverage,
    mp4: Mp4Coverage,
    spout: SpoutCoverage,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureCoverage {
    finished: bool,
    imported: bool,
    reopened: bool,
    latent_slots: u64,
    decoded_frames: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Mp4Coverage {
    finished: bool,
    frames_written: u64,
    byte_length: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpoutCoverage {
    enabled: bool,
    published_frames: u64,
    sender_renamed: bool,
    renamed_published_frames: u64,
    disabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
struct ExternalDeckCoverage {
    opened: bool,
    processed_frames: u64,
    reset_generation_before: u64,
    reset_generation_after: u64,
    reset_confirmed: bool,
    status_checked: bool,
    status_state: String,
    installed_after_runtime_start: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StabilityCoverage {
    surface: String,
    duration_seconds: u64,
    sample_interval_seconds: u64,
    samples: u64,
    worker_faults: u64,
    host_faults: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
struct SafetyEvidence {
    conversion_attempted: bool,
    resize_attempted: bool,
    crop_attempted: bool,
    latent_reencode_attempted: bool,
    hidden_fallback_attempted: bool,
    private_paths_persisted: bool,
}

#[test]
fn production_private_gate_surface_is_generic_protocol2_only() {
    let player = include_str!("../../../latentplayer/src-tauri/src/playback_runtime_v2.rs");
    for required in [
        "start_player_session_v2",
        "Command::PlayerStep",
        "Command::PlayerReset",
        "RuntimeCommand::ConfigureSpout",
        "request_shutdown",
    ] {
        assert!(
            player.contains(required),
            "missing Player P2 seam: {required}"
        );
    }

    let deck = include_str!("../src/generic_deck_runtime.rs");
    for required in [
        "start_deck_session_v2",
        "Command::DeckProcess",
        "CaptureMode::Snapshot",
        "CaptureMode::LiveCapture",
        "RuntimeCommand::RecordingStart",
        "RuntimeCommand::ConfigureSpout",
        "RuntimeCommand::Diagnostics",
        "Command::MetricsGet",
        "pub(crate) async fn recording_stop",
        "RuntimeCommand::Shutdown",
    ] {
        assert!(
            deck.contains(required),
            "missing generic Deck P2 seam: {required}"
        );
    }
    for forbidden in [
        "worker_client::WorkerClient",
        "d2_arguments",
        "q4_arguments",
        "Command::D2",
        "Command::Q4",
    ] {
        assert!(
            !deck.contains(forbidden),
            "legacy Deck worker seam remains in generic runtime: {forbidden}"
        );
    }

    let commands = include_str!("../src/generic_deck_state.rs");
    for required in [
        "deck_generic_open",
        "deck_generic_process_once",
        "deck_generic_reset",
        "deck_generic_capture_start",
        "deck_generic_capture_stop",
        "deck_generic_recording_start",
        "deck_generic_recording_stop",
        "deck_generic_spout_configure",
        "deck_generic_diagnostics_get",
    ] {
        assert!(
            commands.contains(required),
            "missing host command: {required}"
        );
    }

    let adapter =
        include_str!("../../../../codec-host/codecs/h3/src/latentdeck_codec_h3/adapter.py");
    for required in [
        "PACK_ID = \"org.latentdeck.h3\"",
        "PACK_VERSION = \"0.2.0\"",
        "ADAPTER_ID = \"org.latentdeck.h3\"",
        "ADAPTER_VERSION = \"0.2.0\"",
        "Capability.PLAYER",
        "Capability.REALTIME",
        "Capability.RESAMPLE",
        "Capability.SNAPSHOT_CAPTURE",
        "Capability.LIVE_CAPTURE",
        "Capability.RAW_IMPORT",
    ] {
        assert!(
            adapter.contains(required),
            "missing exact H3 P2 identity: {required}"
        );
    }

    let workspace_gate = include_str!("../../../../tools/Check-Workspace.ps1");
    assert!(
        workspace_gate.contains("tools/Test-PrivateProtocol2GpuGate.ps1"),
        "aggregate workspace gate does not run the path-free P2 contract"
    );
}

#[test]
fn private_gpu_stability_resets_player_on_the_final_nonempty_batch() {
    let runner = include_str!("../src/private_protocol2_gpu_e2e_main.rs");
    assert!(
        runner.contains(
            "if step.status.end_of_stream {\n        player_reset(player).await?;\n    }"
        ),
        "the private GPU stability runner must consume the final decoded batch and reset before issuing another player.step"
    );
}

#[test]
fn private_protocol2_receipt_contract_is_closed_and_path_free() {
    let value = valid_receipt();
    validate_receipt_value(value).expect("valid path-free Protocol 2 evidence");
}

#[test]
fn private_protocol2_receipt_rejects_incomplete_or_path_bearing_evidence() {
    let mut short = valid_receipt();
    short["coverage"]["stability"][1]["duration_seconds"] = json!(359);
    assert!(validate_receipt_value(short).is_err());

    let mut p1 = valid_receipt();
    p1["protocol"]["worker_protocol"] = json!(1);
    assert!(validate_receipt_value(p1).is_err());

    let mut missing_capture = valid_receipt();
    missing_capture["coverage"]["q4"]["live_capture"]["reopened"] = json!(false);
    assert!(validate_receipt_value(missing_capture).is_err());

    let mut capture_not_replayed = valid_receipt();
    capture_not_replayed["coverage"]["d2"]["snapshot"]["decoded_frames"] = json!(0);
    assert!(validate_receipt_value(capture_not_replayed).is_err());

    let mut external_not_dynamic = valid_receipt();
    external_not_dynamic["coverage"]["external_deck"]["installed_after_runtime_start"] =
        json!(false);
    assert!(validate_receipt_value(external_not_dynamic).is_err());

    let mut fake_reset = valid_receipt();
    fake_reset["coverage"]["player"]["reset_generation_after"] = json!(1);
    assert!(validate_receipt_value(fake_reset).is_err());

    let mut empty_mp4 = valid_receipt();
    empty_mp4["coverage"]["q4"]["mp4"]["byte_length"] = json!(0);
    assert!(validate_receipt_value(empty_mp4).is_err());

    let mut unrepublished_spout = valid_receipt();
    unrepublished_spout["coverage"]["d2"]["spout"]["renamed_published_frames"] = json!(24);
    assert!(validate_receipt_value(unrepublished_spout).is_err());

    let mut path = valid_receipt();
    path["private_diagnostic"] = json!(r"W:\private\capture.lc");
    assert!(validate_receipt_value(path).is_err());
}

#[test]
#[ignore = "validates an explicit path-free receipt from a private CUDA run; does not start GPU work"]
fn validate_private_protocol2_gpu_gate_receipt() {
    assert_eq!(
        env::var(OPT_IN_ENV).as_deref(),
        Ok("1"),
        "explicit opt-in is required"
    );
    let path = env::var_os(RECEIPT_ENV).expect("private receipt environment variable is required");
    let path = Path::new(&path);
    assert!(
        path.is_absolute() && path.is_file(),
        "receipt must be one absolute regular file"
    );
    let bytes = fs::read(path).expect("read private receipt");
    assert!(
        bytes.len() <= 1024 * 1024,
        "private receipt exceeds the 1 MiB bound"
    );
    let value: Value = serde_json::from_slice(&bytes).expect("private receipt JSON");
    validate_receipt_value(value).expect("private Protocol 2 GPU evidence contract");
}

fn validate_receipt_value(value: Value) -> Result<(), String> {
    reject_paths(&value)?;
    let receipt: GateReceipt = serde_json::from_value(value).map_err(|error| error.to_string())?;
    validate_receipt(&receipt)
}

#[allow(clippy::too_many_lines)]
fn validate_receipt(receipt: &GateReceipt) -> Result<(), String> {
    require(receipt.schema_version == 1, "schema_version")?;
    require(
        receipt.evidence_kind == "latentdeck_private_protocol2_gpu_gate",
        "evidence_kind",
    )?;
    require(receipt.result == "passed", "result")?;
    require(canonical_hex(&receipt.source_commit, 40), "source_commit")?;
    require(!receipt.git_dirty, "git_dirty")?;

    let protocol = &receipt.protocol;
    require(protocol.worker_protocol == 2, "worker_protocol")?;
    require(
        protocol.worker_module == "latentdeck_codec_host",
        "worker_module",
    )?;
    require(protocol.codec_host_api == "2.0", "codec_host_api")?;
    require(
        protocol.codec_manifest_version == "2.0.0",
        "codec_manifest_version",
    )?;
    require(
        protocol.adapter_entrypoint == "latentdeck_codec_h3.adapter:make_adapter",
        "adapter_entrypoint",
    )?;
    require(
        protocol
            .capabilities
            .iter()
            .map(String::as_str)
            .eq(CAPABILITIES),
        "capabilities",
    )?;
    require(!protocol.p1_fallback_attempted, "p1_fallback_attempted")?;

    require_exact(&receipt.packages.codec, CODEC_ID, CODEC_VERSION, "codec")?;
    require_exact(
        &receipt.packages.adapter,
        ADAPTER_ID,
        ADAPTER_VERSION,
        "adapter",
    )?;
    require(
        receipt.packages.decks
            == vec![
                ExactPackage {
                    id: D2_ID.to_owned(),
                    version: DECK_VERSION.to_owned(),
                },
                ExactPackage {
                    id: Q4_ID.to_owned(),
                    version: DECK_VERSION.to_owned(),
                },
            ],
        "decks",
    )?;
    require_exact(
        &receipt.packages.external_deck,
        EXTERNAL_DECK_ID,
        DECK_VERSION,
        "external_deck",
    )?;

    let profile = &receipt.profile;
    require(
        profile.codec_family == PROFILE_FAMILY,
        "profile.codec_family",
    )?;
    require(profile.profile == PROFILE_NAME, "profile.profile")?;
    require(
        profile.profile_version == PROFILE_VERSION,
        "profile.profile_version",
    )?;
    require(profile.python == "3.13", "profile.python")?;
    require(profile.torch == TORCH_BUILD, "profile.torch")?;
    require(
        profile.device == "cuda" && profile.device_ordinal == 0,
        "profile.device",
    )?;

    require(
        canonical_hex(&receipt.inputs.codec_pack_sha256, 64),
        "codec hash",
    )?;
    require(
        canonical_hex(&receipt.inputs.decoder_sha256, 64),
        "decoder hash",
    )?;
    require(
        receipt.inputs.source_archive_sha256.len() == 4
            && receipt
                .inputs
                .source_archive_sha256
                .iter()
                .all(|hash| canonical_hex(hash, 64)),
        "source hashes",
    )?;
    require(
        canonical_hex(&receipt.inputs.external_deck_archive_sha256, 64),
        "external deck hash",
    )?;

    validate_player(&receipt.coverage.player)?;
    validate_deck(&receipt.coverage.d2, "d2")?;
    validate_deck(&receipt.coverage.q4, "q4")?;
    validate_external_deck(&receipt.coverage.external_deck)?;
    let surfaces = receipt
        .coverage
        .stability
        .iter()
        .map(|run| run.surface.as_str())
        .collect::<BTreeSet<_>>();
    require(
        surfaces == BTreeSet::from(["player", "d2", "q4"]),
        "stability surfaces",
    )?;
    require(
        receipt.coverage.stability.len() == 3,
        "duplicate stability surface",
    )?;
    for run in &receipt.coverage.stability {
        require(
            run.duration_seconds >= STABILITY_SECONDS,
            "stability duration",
        )?;
        require(
            (1..=5).contains(&run.sample_interval_seconds),
            "stability interval",
        )?;
        let minimum_samples = STABILITY_SECONDS / run.sample_interval_seconds;
        require(run.samples >= minimum_samples, "stability samples")?;
        require(
            run.worker_faults == 0 && run.host_faults == 0,
            "stability faults",
        )?;
    }

    let safety = &receipt.safety;
    require(
        !safety.conversion_attempted
            && !safety.resize_attempted
            && !safety.crop_attempted
            && !safety.latent_reencode_attempted
            && !safety.hidden_fallback_attempted
            && !safety.private_paths_persisted,
        "safety",
    )
}

fn validate_player(value: &PlayerCoverage) -> Result<(), String> {
    require(
        value.opened
            && value.decoded_frames > 0
            && value.reset_confirmed
            && value.reset_generation_after == value.reset_generation_before.saturating_add(1)
            && value.status_checked
            && healthy_player_status(&value.status_state),
        "player coverage",
    )?;
    validate_spout(&value.spout, "player spout")
}

fn validate_deck(value: &DeckCoverage, label: &str) -> Result<(), String> {
    require(
        value.opened
            && value.processed_frames > 0
            && value.reset_confirmed
            && value.reset_generation_after == value.reset_generation_before.saturating_add(1)
            && value.status_checked
            && healthy_deck_status(&value.status_state),
        label,
    )?;
    validate_capture(&value.snapshot, "snapshot")?;
    validate_capture(&value.live_capture, "live_capture")?;
    require(
        value.mp4.finished && value.mp4.frames_written > 0 && value.mp4.byte_length > 0,
        "mp4",
    )?;
    validate_spout(&value.spout, "deck spout")
}

fn validate_capture(value: &CaptureCoverage, label: &str) -> Result<(), String> {
    require(
        value.finished && value.imported && value.reopened && value.latent_slots > 0,
        label,
    )?;
    require(value.decoded_frames > 0, "capture replay")
}

fn validate_spout(value: &SpoutCoverage, label: &str) -> Result<(), String> {
    require(
        value.enabled
            && value.published_frames > 0
            && value.sender_renamed
            && value.renamed_published_frames > value.published_frames
            && value.disabled,
        label,
    )
}

fn validate_external_deck(value: &ExternalDeckCoverage) -> Result<(), String> {
    require(
        value.opened
            && value.processed_frames > 0
            && value.reset_confirmed
            && value.reset_generation_after == value.reset_generation_before.saturating_add(1)
            && value.status_checked
            && healthy_deck_status(&value.status_state)
            && value.installed_after_runtime_start,
        "external deck coverage",
    )
}

fn healthy_player_status(value: &str) -> bool {
    matches!(value, "ready" | "playing" | "paused" | "end_of_stream")
}

fn healthy_deck_status(value: &str) -> bool {
    matches!(value, "ready" | "playing" | "paused")
}

fn require_exact(value: &ExactPackage, id: &str, version: &str, label: &str) -> Result<(), String> {
    require(value.id == id && value.version == version, label)
}

fn require(condition: bool, label: &str) -> Result<(), String> {
    condition
        .then_some(())
        .ok_or_else(|| format!("invalid {label}"))
}

fn canonical_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn reject_paths(value: &Value) -> Result<(), String> {
    match value {
        Value::String(text) if path_like(text) => {
            Err("receipt contains a machine-local path".to_owned())
        }
        Value::Array(values) => values.iter().try_for_each(reject_paths),
        Value::Object(values) => values.values().try_for_each(reject_paths),
        _ => Ok(()),
    }
}

fn path_like(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.starts_with('/')
        || value.starts_with(r"\\")
        || lower.contains(":\\")
        || lower.contains(":/")
        || lower.contains("file://")
        || lower.contains("%localappdata%")
}

#[allow(clippy::too_many_lines)]
fn valid_receipt() -> Value {
    let hash = "a".repeat(64);
    let spout = || {
        json!({
            "enabled": true,
            "published_frames": 24,
            "sender_renamed": true,
            "renamed_published_frames": 25,
            "disabled": true
        })
    };
    let capture = || {
        json!({
            "finished": true,
            "imported": true,
            "reopened": true,
            "latent_slots": 2,
            "decoded_frames": 24
        })
    };
    let deck = || {
        json!({
            "opened": true,
            "processed_frames": 144,
            "reset_generation_before": 1,
            "reset_generation_after": 2,
            "reset_confirmed": true,
            "status_checked": true,
            "status_state": "ready",
            "snapshot": capture(),
            "live_capture": capture(),
            "mp4": {"finished": true, "frames_written": 120, "byte_length": 4096},
            "spout": spout()
        })
    };
    json!({
        "schema_version": 1,
        "evidence_kind": "latentdeck_private_protocol2_gpu_gate",
        "result": "passed",
        "source_commit": "b".repeat(40),
        "git_dirty": false,
        "protocol": {
            "worker_protocol": 2,
            "worker_module": "latentdeck_codec_host",
            "codec_host_api": "2.0",
            "codec_manifest_version": "2.0.0",
            "adapter_entrypoint": "latentdeck_codec_h3.adapter:make_adapter",
            "capabilities": CAPABILITIES,
            "p1_fallback_attempted": false
        },
        "packages": {
            "codec": {"id": CODEC_ID, "version": CODEC_VERSION},
            "adapter": {"id": ADAPTER_ID, "version": ADAPTER_VERSION},
            "decks": [
                {"id": D2_ID, "version": DECK_VERSION},
                {"id": Q4_ID, "version": DECK_VERSION}
            ],
            "external_deck": {"id": EXTERNAL_DECK_ID, "version": DECK_VERSION}
        },
        "profile": {
            "codec_family": PROFILE_FAMILY,
            "profile": PROFILE_NAME,
            "profile_version": PROFILE_VERSION,
            "python": "3.13",
            "torch": TORCH_BUILD,
            "device": "cuda",
            "device_ordinal": 0
        },
        "inputs": {
            "codec_pack_sha256": hash,
            "decoder_sha256": "c".repeat(64),
            "source_archive_sha256": ["d".repeat(64), "e".repeat(64), "f".repeat(64), "1".repeat(64)],
            "external_deck_archive_sha256": "2".repeat(64)
        },
        "coverage": {
            "player": {
                "opened": true,
                "decoded_frames": 48,
                "reset_generation_before": 1,
                "reset_generation_after": 2,
                "reset_confirmed": true,
                "status_checked": true,
                "status_state": "ready",
                "spout": spout()
            },
            "d2": deck(),
            "q4": deck(),
            "external_deck": {
                "opened": true,
                "processed_frames": 24,
                "reset_generation_before": 1,
                "reset_generation_after": 2,
                "reset_confirmed": true,
                "status_checked": true,
                "status_state": "ready",
                "installed_after_runtime_start": true
            },
            "stability": [
                {"surface": "player", "duration_seconds": 360, "sample_interval_seconds": 5, "samples": 72, "worker_faults": 0, "host_faults": 0},
                {"surface": "d2", "duration_seconds": 360, "sample_interval_seconds": 5, "samples": 72, "worker_faults": 0, "host_faults": 0},
                {"surface": "q4", "duration_seconds": 360, "sample_interval_seconds": 5, "samples": 72, "worker_faults": 0, "host_faults": 0}
            ]
        },
        "safety": {
            "conversion_attempted": false,
            "resize_attempted": false,
            "crop_attempted": false,
            "latent_reencode_attempted": false,
            "hidden_fallback_attempted": false,
            "private_paths_persisted": false
        }
    })
}
