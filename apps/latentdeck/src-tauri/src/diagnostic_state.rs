//! LatentDeck-only host for bounded realtime support bundles.

use std::{path::Path, sync::Mutex};

use latentdeck_control::WORKER_PROTOCOL_VERSION;
use latentdeck_core::realtime_diagnostics::{
    DiagnosticBundleInput, DiagnosticBundleReceipt, DiagnosticCodecIdentity,
    DiagnosticCollectionLimits, DiagnosticEventSource, DiagnosticGpuIdentity, DiagnosticLogSource,
    DiagnosticProduct, DiagnosticProductIdentity, InactiveApplicationDiagnosticSession,
    RealtimeDiagnosticError, RealtimeDiagnosticSession, RealtimeDiagnosticSnapshot, SanitizedToken,
    collect_diagnostic_events, write_diagnostic_bundle_atomic,
};
use serde::Serialize;

use crate::{d2_runtime::D2RuntimeDiagnostics, q4_runtime::Q4RuntimeDiagnostics};

const CODEC_FAMILY: &str = "minimax_h3";
const PROFILE: &str = "h3_av_latent";
const UNAVAILABLE: &str = "unavailable";
const RUNTIME: &str = "worker_protocol";

#[derive(Default)]
struct LifecycleState {
    last_error: Option<SanitizedToken>,
    capture_failed: bool,
}

/// Process-wide stable failure memory shared by D2 and Q4 lifecycles.
///
/// Only validated codes enter this state. A poisoned lock or unsafe token makes
/// diagnostic capture fail closed rather than dropping a known error.
#[derive(Default)]
pub(crate) struct DeckDiagnosticLifecycle {
    inner: Mutex<LifecycleState>,
}

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

#[derive(Debug)]
pub(crate) enum DeckSnapshotError {
    Contract(RealtimeDiagnosticError),
    IdentityConflict,
}

impl From<RealtimeDiagnosticError> for DeckSnapshotError {
    fn from(error: RealtimeDiagnosticError) -> Self {
        Self::Contract(error)
    }
}

impl DeckDiagnosticLifecycle {
    pub(crate) fn record_error(&self, code: &str) {
        let parsed = SanitizedToken::parse(code);
        let Ok(mut state) = self.inner.lock() else {
            return;
        };
        match parsed {
            Ok(code) => state.last_error = Some(code),
            Err(_) => state.capture_failed = true,
        }
    }

    pub(crate) fn last_error(&self) -> Result<Option<SanitizedToken>, RealtimeDiagnosticError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| RealtimeDiagnosticError::InvalidToken)?;
        if state.capture_failed {
            return Err(RealtimeDiagnosticError::InvalidToken);
        }
        Ok(state.last_error.clone())
    }
}

pub(crate) fn inactive_snapshot(
    captured_at_unix_ms: u64,
    last_error: Option<SanitizedToken>,
) -> Result<RealtimeDiagnosticSnapshot, RealtimeDiagnosticError> {
    let lifecycle = match last_error {
        Some(code) => InactiveApplicationDiagnosticSession::with_last_error(code),
        None => InactiveApplicationDiagnosticSession::new(),
    };
    RealtimeDiagnosticSnapshot::new(
        captured_at_unix_ms,
        product_identity()?,
        DiagnosticGpuIdentity::new(token(UNAVAILABLE)?, token(UNAVAILABLE)?),
        DiagnosticCodecIdentity::new(
            token(CODEC_FAMILY)?,
            token(PROFILE)?,
            token(UNAVAILABLE)?,
            token(UNAVAILABLE)?,
            token(UNAVAILABLE)?,
            None,
        ),
        RealtimeDiagnosticSession::NoActiveSession(lifecycle),
    )
}

pub(crate) fn deck_snapshot(
    captured_at_unix_ms: u64,
    d2: Option<D2RuntimeDiagnostics>,
    q4: Option<Q4RuntimeDiagnostics>,
    last_error: Option<SanitizedToken>,
) -> Result<RealtimeDiagnosticSnapshot, DeckSnapshotError> {
    match (d2, q4) {
        (None, None) => Ok(inactive_snapshot(captured_at_unix_ms, last_error)?),
        (Some(d2), None) => Ok(RealtimeDiagnosticSnapshot::new(
            captured_at_unix_ms,
            product_identity()?,
            d2.gpu,
            d2.codec,
            RealtimeDiagnosticSession::DeckD2(d2.session),
        )?),
        (None, Some(q4)) => Ok(RealtimeDiagnosticSnapshot::new(
            captured_at_unix_ms,
            product_identity()?,
            q4.gpu,
            q4.codec,
            RealtimeDiagnosticSession::DeckQ4(q4.session),
        )?),
        (Some(d2), Some(q4)) => {
            if d2.gpu != q4.gpu || d2.codec != q4.codec {
                return Err(DeckSnapshotError::IdentityConflict);
            }
            Ok(RealtimeDiagnosticSnapshot::new(
                captured_at_unix_ms,
                product_identity()?,
                d2.gpu,
                d2.codec,
                RealtimeDiagnosticSession::DeckD2(d2.session),
            )?
            .with_session(RealtimeDiagnosticSession::DeckQ4(q4.session))?)
        }
    }
}

/// Collect only installed Deck and worker lifecycle roots, then atomically
/// publish the Core-owned exact three-entry archive.
pub(crate) fn write_deck_bundle(
    destination: &Path,
    snapshot: &RealtimeDiagnosticSnapshot,
    deck_log_root: &Path,
    worker_log_root: &Path,
) -> Result<DiagnosticBundleReceipt, RealtimeDiagnosticError> {
    let sources = [
        DiagnosticLogSource::new(DiagnosticEventSource::Deck, deck_log_root),
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
        DiagnosticProduct::LatentDeck,
        token(latentdeck_core::product_version())?,
        token(RUNTIME)?,
        token(&WORKER_PROTOCOL_VERSION.to_string())?,
    ))
}

fn token(value: &str) -> Result<SanitizedToken, RealtimeDiagnosticError> {
    SanitizedToken::parse(value)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use latentdeck_control::MetricsSnapshot;
    use latentdeck_core::realtime_diagnostics::{
        D2DiagnosticSession, PresentationDiagnosticCounters, Q4DiagnosticSession,
        RealtimeSessionMetrics, Sha256Token, TimingDistribution, WorkerDiagnosticCounters,
    };
    use serde_json::Value;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn inactive_snapshot_declares_no_active_session_without_realtime_metrics() {
        let snapshot = inactive_snapshot(1_800_000_000_000, None).expect("inactive snapshot");
        let value: Value = serde_json::to_value(snapshot).expect("serialize snapshot");

        assert_eq!(value["product"]["product"], "latent_deck");
        assert_eq!(value["inactive_application"]["no_active_session"], true);
        assert!(value.get("deck_d2").is_none());
        assert!(value.get("deck_q4").is_none());
    }

    #[test]
    fn combined_snapshot_requires_and_preserves_one_exact_identity() {
        let gpu = gpu_identity("gpu");
        let codec = codec_identity();
        let snapshot = deck_snapshot(
            1_800_000_000_000,
            Some(d2_diagnostics(gpu.clone(), codec.clone())),
            Some(q4_diagnostics(gpu, codec)),
            None,
        )
        .expect("combined snapshot");
        let value: Value = serde_json::to_value(snapshot).expect("serialize");

        assert!(value.get("deck_d2").is_some());
        assert!(value.get("deck_q4").is_some());
        assert!(value.get("inactive_application").is_none());
        assert_eq!(value["deck_q4"]["carrier_slot"], 2);
    }

    #[test]
    fn combined_snapshot_fails_closed_on_gpu_or_codec_identity_mismatch() {
        let gpu_error = deck_snapshot(
            1_800_000_000_000,
            Some(d2_diagnostics(gpu_identity("gpu-a"), codec_identity())),
            Some(q4_diagnostics(gpu_identity("gpu-b"), codec_identity())),
            None,
        )
        .expect_err("mixed identities must not be merged");

        assert!(matches!(gpu_error, DeckSnapshotError::IdentityConflict));

        let codec_error = deck_snapshot(
            1_800_000_000_000,
            Some(d2_diagnostics(gpu_identity("gpu"), codec_identity())),
            Some(q4_diagnostics(
                gpu_identity("gpu"),
                DiagnosticCodecIdentity::new(
                    token(CODEC_FAMILY).expect("family"),
                    token(PROFILE).expect("profile"),
                    token("org.latentdeck.h3").expect("pack"),
                    token("0.1.0").expect("version"),
                    token("taeh3").expect("decoder"),
                    Some(hash("ff")),
                ),
            )),
            None,
        )
        .expect_err("mixed codec identity must not be merged");
        assert!(matches!(codec_error, DeckSnapshotError::IdentityConflict));
    }

    #[test]
    fn inactive_lifecycle_keeps_only_the_last_stable_error_code() {
        let lifecycle = DeckDiagnosticLifecycle::default();
        lifecycle.record_error("deck.runtime_unavailable");
        lifecycle.record_error("worker.exited");
        let snapshot = deck_snapshot(
            1_800_000_000_000,
            None,
            None,
            lifecycle.last_error().expect("stable lifecycle"),
        )
        .expect("snapshot");
        let value: Value = serde_json::to_value(snapshot).expect("serialize");

        assert_eq!(
            value["inactive_application"]["last_error_code"],
            "worker.exited"
        );
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
    fn deck_bundle_has_exact_entries_and_reads_only_deck_and_worker_roots() {
        let test_root = std::env::temp_dir().join(format!(
            "latentdeck-diagnostics-test-{}",
            Uuid::new_v4().simple()
        ));
        let deck_root = test_root.join("deck");
        let worker_root = test_root.join("worker");
        let player_root = test_root.join("player");
        fs::create_dir_all(&deck_root).expect("deck root");
        fs::create_dir_all(&worker_root).expect("worker root");
        fs::create_dir_all(&player_root).expect("player root");
        fs::write(
            deck_root.join("latentdeck-1.jsonl"),
            "{\"schema_version\":1,\"timestamp_unix_ms\":1800000000000,\"level\":\"info\",\"event\":\"app.started\"}\n",
        )
        .expect("deck log");
        fs::write(
            worker_root.join("worker-1.jsonl"),
            "{\"schema_version\":1,\"timestamp_ns\":1800000000001000000,\"event\":\"decode_started\"}\n",
        )
        .expect("worker log");
        fs::write(
            player_root.join("latentplayer-1.jsonl"),
            "{\"schema_version\":1,\"timestamp_unix_ms\":1800000000002,\"level\":\"error\",\"event\":\"player.must_not_appear\"}\n",
        )
        .expect("player log");

        let output = test_root.join("bundle.zip");
        let snapshot = inactive_snapshot(1_800_000_000_003, None).expect("snapshot");
        let receipt =
            write_deck_bundle(&output, &snapshot, &deck_root, &worker_root).expect("bundle");
        let archive = fs::read(&output).expect("archive");
        let archive_text = String::from_utf8_lossy(&archive);

        assert_eq!(receipt.event_count, 2);
        assert!(archive_text.contains("app.started"));
        assert!(archive_text.contains("decode_started"));
        assert!(!archive_text.contains("player.must_not_appear"));
        assert_eq!(
            central_directory_names(&archive),
            ["manifest.json", "events.jsonl", "realtime.json"]
        );
        assert!(matches!(
            write_deck_bundle(&output, &snapshot, &deck_root, &worker_root),
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

    fn d2_diagnostics(
        gpu: DiagnosticGpuIdentity,
        codec: DiagnosticCodecIdentity,
    ) -> D2RuntimeDiagnostics {
        D2RuntimeDiagnostics {
            gpu,
            codec,
            session: D2DiagnosticSession::new(
                token("org.latentdeck.builtin.ld_d2").expect("operator"),
                [hash("aa"), hash("bb")],
                metrics(),
            ),
        }
    }

    fn q4_diagnostics(
        gpu: DiagnosticGpuIdentity,
        codec: DiagnosticCodecIdentity,
    ) -> Q4RuntimeDiagnostics {
        Q4RuntimeDiagnostics {
            gpu,
            codec,
            session: Q4DiagnosticSession::new(
                token("org.latentdeck.builtin.ld_q4").expect("operator"),
                2,
                [hash("aa"), hash("bb"), hash("cc"), hash("dd")],
                metrics(),
            )
            .expect("Q4 session"),
        }
    }

    fn gpu_identity(adapter: &str) -> DiagnosticGpuIdentity {
        DiagnosticGpuIdentity::new(
            token(adapter).expect("adapter"),
            token("driver").expect("driver"),
        )
    }

    fn codec_identity() -> DiagnosticCodecIdentity {
        DiagnosticCodecIdentity::new(
            token(CODEC_FAMILY).expect("family"),
            token(PROFILE).expect("profile"),
            token("org.latentdeck.h3").expect("pack"),
            token("0.1.0").expect("version"),
            token("taeh3").expect("decoder"),
            Some(hash("ee")),
        )
    }

    fn hash(byte: &str) -> Sha256Token {
        Sha256Token::parse(&byte.repeat(32)).expect("hash")
    }

    fn metrics() -> RealtimeSessionMetrics {
        let timing = TimingDistribution::new(0, 0.0, 0.0, 0.0, 0.0).expect("timing");
        RealtimeSessionMetrics::new(
            1_000,
            24.0,
            0.0,
            timing,
            timing,
            WorkerDiagnosticCounters::from_metrics_snapshot(&MetricsSnapshot {
                worker_uptime_ns: 0,
                decode_batches_total: 0,
                decoded_frames_total: 0,
                ring_backpressure_total: 0,
                presentation_skipped_total: 0,
                last_decode_duration_ns: 0,
                ring_write_sequence: 0,
                ring_read_sequence: 0,
                ring_occupancy: 0,
                gpu_allocated_bytes: None,
                gpu_reserved_bytes: None,
            })
            .expect("worker"),
            PresentationDiagnosticCounters::new(0, None, Some(0)).expect("presentation"),
            Vec::new(),
        )
        .expect("metrics")
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
}
