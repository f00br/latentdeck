//! LatentPlayer-only host for bounded realtime support bundles.

use std::path::Path;

use latentdeck_control::WORKER_PROTOCOL_VERSION;
use latentdeck_core::{
    player::PlayerView,
    realtime_diagnostics::{
        DiagnosticBundleInput, DiagnosticBundleReceipt, DiagnosticCodecIdentity,
        DiagnosticCollectionLimits, DiagnosticEventSource, DiagnosticGpuIdentity,
        DiagnosticLogSource, DiagnosticProduct, DiagnosticProductIdentity,
        InactiveApplicationDiagnosticSession, RealtimeDiagnosticError, RealtimeDiagnosticSession,
        RealtimeDiagnosticSnapshot, SanitizedToken, Sha256Token, collect_diagnostic_events,
        write_diagnostic_bundle_atomic,
    },
};
use serde::Serialize;

use crate::playback_runtime::PlaybackRuntimeDiagnostics;

const CODEC_FAMILY: &str = "minimax_h3";
const PROFILE: &str = "h3_av_latent";
const MISSING: &str = "missing";
const UNAVAILABLE: &str = "unavailable";
const RUNTIME: &str = "worker_protocol";

/// Path-free native command result returned to the webview.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum DiagnosticSaveResult {
    Saved {
        #[serde(rename = "archiveBytes")]
        archive_bytes: u64,
        #[serde(rename = "eventCount")]
        event_count: usize,
        #[serde(rename = "schemaVersion")]
        schema_version: u16,
    },
    Cancelled,
}

impl From<DiagnosticBundleReceipt> for DiagnosticSaveResult {
    fn from(receipt: DiagnosticBundleReceipt) -> Self {
        Self::Saved {
            archive_bytes: receipt.archive_bytes,
            event_count: receipt.event_count,
            schema_version: receipt.schema_version,
        }
    }
}

/// Build the active Player form from exact runtime identities and counters.
pub(crate) fn active_snapshot(
    captured_at_unix_ms: u64,
    diagnostics: PlaybackRuntimeDiagnostics,
) -> Result<RealtimeDiagnosticSnapshot, RealtimeDiagnosticError> {
    RealtimeDiagnosticSnapshot::new(
        captured_at_unix_ms,
        product_identity()?,
        diagnostics.gpu,
        diagnostics.codec,
        RealtimeDiagnosticSession::Player(diagnostics.session),
    )
}

/// Build the truthful lifecycle-only form when no realtime actor is active.
pub(crate) fn inactive_snapshot(
    captured_at_unix_ms: u64,
    player: &PlayerView,
) -> Result<RealtimeDiagnosticSnapshot, RealtimeDiagnosticError> {
    let lifecycle = match player.error.as_ref() {
        Some(error) => InactiveApplicationDiagnosticSession::with_last_error(token(&error.code)?),
        None => InactiveApplicationDiagnosticSession::new(),
    };
    RealtimeDiagnosticSnapshot::new(
        captured_at_unix_ms,
        product_identity()?,
        DiagnosticGpuIdentity::new(token(UNAVAILABLE)?, token(UNAVAILABLE)?),
        inactive_codec_identity(player)?,
        RealtimeDiagnosticSession::NoActiveSession(lifecycle),
    )
}

/// Collect only installed Player and worker lifecycle roots, then atomically
/// publish the Core-owned exact three-entry archive.
pub(crate) fn write_player_bundle(
    destination: &Path,
    snapshot: &RealtimeDiagnosticSnapshot,
    player_log_root: &Path,
    worker_log_root: &Path,
) -> Result<DiagnosticBundleReceipt, RealtimeDiagnosticError> {
    let sources = [
        DiagnosticLogSource::new(DiagnosticEventSource::Player, player_log_root),
        DiagnosticLogSource::new(DiagnosticEventSource::Worker, worker_log_root),
    ];
    let collection = collect_diagnostic_events(&sources, DiagnosticCollectionLimits::default())?;
    write_diagnostic_bundle_atomic(
        destination,
        DiagnosticBundleInput::from_collection(snapshot, &collection),
    )
}

fn product_identity() -> Result<DiagnosticProductIdentity, RealtimeDiagnosticError> {
    Ok(DiagnosticProductIdentity::new(
        DiagnosticProduct::LatentPlayer,
        token(latentdeck_core::product_version())?,
        token(RUNTIME)?,
        token(&WORKER_PROTOCOL_VERSION.to_string())?,
    ))
}

fn inactive_codec_identity(
    player: &PlayerView,
) -> Result<DiagnosticCodecIdentity, RealtimeDiagnosticError> {
    let decoder_sha256 = player
        .codec
        .decoder_variants
        .iter()
        .find(|variant| variant.selected)
        .map(|variant| Sha256Token::parse(&variant.sha256))
        .transpose()?;
    Ok(DiagnosticCodecIdentity::new(
        token(CODEC_FAMILY)?,
        token(PROFILE)?,
        token(player.codec.pack_id.as_deref().unwrap_or(MISSING))?,
        token(player.codec.pack_version.as_deref().unwrap_or(MISSING))?,
        token(player.codec.decoder_asset_id.as_deref().unwrap_or(MISSING))?,
        decoder_sha256,
    ))
}

fn token(value: &str) -> Result<SanitizedToken, RealtimeDiagnosticError> {
    SanitizedToken::parse(value)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value;
    use uuid::Uuid;

    use super::*;
    use latentdeck_control::MetricsSnapshot;
    use latentdeck_core::player::{CodecState, CodecSummary, DecoderVariantSummary, PlayerPhase};
    use latentdeck_core::realtime_diagnostics::{
        PlayerDiagnosticSession, PresentationDiagnosticCounters, RealtimeSessionMetrics,
        TimingDistribution, WorkerDiagnosticCounters,
    };

    #[test]
    fn inactive_snapshot_is_explicit_and_contains_no_fake_player_metrics() {
        let snapshot = inactive_snapshot(1_800_000_000_000, &empty_view()).expect("snapshot");
        let value = serde_json::to_value(snapshot).expect("serialize");

        assert_eq!(value["inactive_application"]["no_active_session"], true);
        assert!(value.get("player").is_none());
        assert_eq!(value["gpu"]["adapter"], UNAVAILABLE);
        assert_eq!(value["codec"]["codec_pack"], MISSING);
    }

    #[test]
    fn ended_failed_session_keeps_only_the_stable_error_code() {
        let mut player = empty_view();
        player.error = Some(latentdeck_core::player::PlayerErrorView {
            code: "worker.exited".to_owned(),
            message: "A private backend detail must not enter the snapshot".to_owned(),
            recoverable: true,
        });

        let value =
            serde_json::to_value(inactive_snapshot(1_800_000_000_000, &player).expect("snapshot"))
                .expect("serialize");

        assert_eq!(
            value["inactive_application"]["last_error_code"],
            "worker.exited"
        );
        assert!(!value.to_string().contains("private backend"));
        assert!(value.get("player").is_none());
    }

    #[test]
    fn active_snapshot_contains_the_player_session_and_exact_worker_counters() {
        let worker_metrics = MetricsSnapshot {
            worker_uptime_ns: 90,
            decode_batches_total: 7,
            decoded_frames_total: 107,
            ring_backpressure_total: 3,
            presentation_skipped_total: 2,
            last_decode_duration_ns: 11,
            ring_write_sequence: 108,
            ring_read_sequence: 106,
            ring_occupancy: 2,
            gpu_allocated_bytes: Some(1_024),
            gpu_reserved_bytes: Some(2_048),
        };
        let zero_timing = TimingDistribution::new(0, 0.0, 0.0, 0.0, 0.0).expect("zero timing");
        let metrics = RealtimeSessionMetrics::new(
            1_000,
            24.0,
            23.98,
            zero_timing,
            zero_timing,
            WorkerDiagnosticCounters::from_metrics_snapshot(&worker_metrics)
                .expect("worker counters"),
            PresentationDiagnosticCounters::new(105, Some(2), Some(100))
                .expect("presentation counters"),
            Vec::new(),
        )
        .expect("realtime metrics");
        let diagnostics = PlaybackRuntimeDiagnostics {
            gpu: DiagnosticGpuIdentity::new(
                token("gpu").expect("gpu"),
                token("driver").expect("driver"),
            ),
            codec: DiagnosticCodecIdentity::new(
                token(CODEC_FAMILY).expect("family"),
                token(PROFILE).expect("profile"),
                token("org.latentdeck.h3").expect("pack"),
                token("0.1.0").expect("pack version"),
                token("taeh3").expect("decoder"),
                Some(Sha256Token::parse(&"ab".repeat(32)).expect("decoder hash")),
            ),
            session: PlayerDiagnosticSession::new(
                Sha256Token::parse(&"cd".repeat(32)).expect("cartridge hash"),
                metrics,
            ),
        };

        let value = serde_json::to_value(
            active_snapshot(1_800_000_000_000, diagnostics).expect("snapshot"),
        )
        .expect("serialize");
        assert!(value.get("inactive_application").is_none());
        assert_eq!(
            value["player"]["metrics"]["worker"]["decode_batches_total"],
            7
        );
        assert_eq!(value["player"]["metrics"]["worker"]["ring_occupancy"], 2);
        assert_eq!(value["player"]["cartridge_sha256"], "cd".repeat(32));
    }

    #[test]
    fn save_receipt_never_serializes_the_native_destination() {
        let value = serde_json::to_value(DiagnosticSaveResult::from(DiagnosticBundleReceipt {
            archive_bytes: 4_096,
            event_count: 3,
            schema_version: 1,
        }))
        .expect("serialize");

        assert_eq!(value["status"], "saved");
        assert_eq!(value["archiveBytes"], 4_096);
        assert!(value.get("path").is_none());
        assert!(value.get("destination").is_none());
    }

    #[test]
    fn player_bundle_reads_only_player_and_worker_roots() {
        let test_root = std::env::temp_dir().join(format!(
            "latentplayer-diagnostics-test-{}",
            Uuid::new_v4().simple()
        ));
        let player_root = test_root.join("player");
        let worker_root = test_root.join("worker");
        let deck_root = test_root.join("deck");
        fs::create_dir_all(&player_root).expect("player root");
        fs::create_dir_all(&worker_root).expect("worker root");
        fs::create_dir_all(&deck_root).expect("deck root");
        fs::write(
            player_root.join("latentplayer-1.jsonl"),
            "{\"schema_version\":1,\"timestamp_unix_ms\":1800000000000,\"level\":\"info\",\"event\":\"app.started\"}\n",
        )
        .expect("player log");
        fs::write(
            worker_root.join("worker-1.jsonl"),
            "{\"schema_version\":1,\"timestamp_ns\":1800000000001000000,\"event\":\"decode_started\"}\n",
        )
        .expect("worker log");
        fs::write(
            deck_root.join("latentdeck-1.jsonl"),
            "{\"schema_version\":1,\"timestamp_unix_ms\":1800000000002,\"level\":\"error\",\"event\":\"deck.must_not_appear\"}\n",
        )
        .expect("deck log");

        let output = test_root.join("bundle.zip");
        let snapshot = inactive_snapshot(1_800_000_000_003, &empty_view()).expect("snapshot");
        let receipt =
            write_player_bundle(&output, &snapshot, &player_root, &worker_root).expect("bundle");
        let archive = fs::read(&output).expect("archive");
        let archive_text = String::from_utf8_lossy(&archive);

        assert_eq!(receipt.event_count, 2);
        assert!(archive_text.contains("app.started"));
        assert!(archive_text.contains("decode_started"));
        assert!(!archive_text.contains("deck.must_not_appear"));
        assert!(archive_text.contains("manifest.json"));
        assert!(archive_text.contains("events.jsonl"));
        assert!(archive_text.contains("realtime.json"));
        assert_eq!(
            central_directory_names(&archive),
            ["manifest.json", "events.jsonl", "realtime.json"]
        );
        assert!(matches!(
            write_player_bundle(&output, &snapshot, &player_root, &worker_root),
            Err(RealtimeDiagnosticError::OutputExists)
        ));
        assert!(
            fs::read_dir(&test_root)
                .expect("test root inventory")
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".partial"))
        );

        fs::remove_dir_all(&test_root).expect("remove isolated test root");
    }

    fn central_directory_names(archive: &[u8]) -> Vec<String> {
        const CENTRAL_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
        const CENTRAL_HEADER_BYTES: usize = 46;
        let mut names = Vec::new();
        let mut cursor = 0;
        while cursor + CENTRAL_HEADER_BYTES <= archive.len() {
            if archive[cursor..cursor + 4] != CENTRAL_SIGNATURE {
                cursor += 1;
                continue;
            }
            let name_length = usize::from(u16::from_le_bytes([
                archive[cursor + 28],
                archive[cursor + 29],
            ]));
            let extra_length = usize::from(u16::from_le_bytes([
                archive[cursor + 30],
                archive[cursor + 31],
            ]));
            let comment_length = usize::from(u16::from_le_bytes([
                archive[cursor + 32],
                archive[cursor + 33],
            ]));
            let name_start = cursor + CENTRAL_HEADER_BYTES;
            let Some(name_end) = name_start.checked_add(name_length) else {
                break;
            };
            if name_end > archive.len() {
                break;
            }
            if let Ok(name) = std::str::from_utf8(&archive[name_start..name_end]) {
                names.push(name.to_owned());
            }
            let Some(next) = name_end
                .checked_add(extra_length)
                .and_then(|value| value.checked_add(comment_length))
            else {
                break;
            };
            cursor = next;
        }
        names
    }

    fn empty_view() -> PlayerView {
        PlayerView {
            revision: 0,
            phase: PlayerPhase::Empty,
            cartridge: None,
            codec: CodecSummary {
                state: CodecState::Missing,
                display_name: None,
                detail: None,
                pack_id: None,
                pack_version: None,
                publisher_name: None,
                publisher_url: None,
                pack_license_label: None,
                decoder_asset_id: None,
                decoder_display_name: None,
                decoder_variants: Vec::new(),
            },
            position_frame: 0,
            loop_enabled: false,
            output_available: false,
            error: None,
        }
    }

    #[test]
    fn inactive_codec_preserves_the_selected_decoder_hash() {
        let mut player = empty_view();
        player.codec.decoder_asset_id = Some("taeh3".to_owned());
        player.codec.decoder_variants.push(DecoderVariantSummary {
            variant_id: "official".to_owned(),
            sha256: "ab".repeat(32),
            byte_length: 1,
            source_url: "https://example.invalid".to_owned(),
            license_label: "Apache-2.0".to_owned(),
            license_url: "https://example.invalid/license".to_owned(),
            selected: true,
        });

        let value: Value =
            serde_json::to_value(inactive_snapshot(1_800_000_000_000, &player).expect("snapshot"))
                .expect("serialize");
        assert_eq!(value["codec"]["decoder"], "taeh3");
        assert_eq!(value["codec"]["decoder_sha256"], "ab".repeat(32));
    }
}
