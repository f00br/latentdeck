//! Codec-neutral host for bounded `LatentDeck` realtime support bundles.

use std::path::Path;

use latentdeck_control::v2::PROTOCOL_VERSION;
use latentdeck_core::realtime_diagnostics::{
    DiagnosticBundleInput, DiagnosticBundleReceipt, DiagnosticCodecIdentity,
    DiagnosticCollectionLimits, DiagnosticEventSource, DiagnosticGpuIdentity, DiagnosticLogSource,
    DiagnosticProduct, DiagnosticProductIdentity, GenericDeckDiagnosticSession,
    InactiveApplicationDiagnosticSession, RealtimeDiagnosticError, RealtimeDiagnosticSession,
    RealtimeDiagnosticSnapshot, SanitizedToken, collect_diagnostic_events,
    write_diagnostic_bundle_atomic,
};
use serde::Serialize;

use crate::generic_deck_runtime::GenericDeckRuntimeDiagnostics;

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
            token(UNAVAILABLE)?,
            token(UNAVAILABLE)?,
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
    diagnostics: Option<GenericDeckRuntimeDiagnostics>,
    last_error: Option<SanitizedToken>,
) -> Result<RealtimeDiagnosticSnapshot, RealtimeDiagnosticError> {
    let Some(diagnostics) = diagnostics else {
        return inactive_snapshot(captured_at_unix_ms, last_error);
    };
    let session = GenericDeckDiagnosticSession::new(
        diagnostics.operator,
        diagnostics.source_archive_sha256,
        diagnostics.metrics,
        diagnostics.session,
    )?;
    RealtimeDiagnosticSnapshot::new(
        captured_at_unix_ms,
        product_identity()?,
        diagnostics.gpu,
        diagnostics.codec,
        RealtimeDiagnosticSession::Deck(session),
    )
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
        token(&PROTOCOL_VERSION.to_string())?,
    ))
}

fn token(value: &str) -> Result<SanitizedToken, RealtimeDiagnosticError> {
    SanitizedToken::parse(value)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn inactive_snapshot_is_codec_neutral_and_path_free() {
        let snapshot = inactive_snapshot(1_800_000_000_000, None).expect("inactive snapshot");
        let value: Value = serde_json::to_value(snapshot).expect("serialize snapshot");

        assert_eq!(value["product"]["product"], "latent_deck");
        assert_eq!(value["inactive_application"]["no_active_session"], true);
        assert!(value.get("deck").is_none());
        assert!(value.get("deck_d2").is_none());
        assert!(value.get("deck_q4").is_none());
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
}
