//! UI-independent `LatentPlayer` media and codec selection state.

use std::path::{Path, PathBuf};

use latentdeck_cartridge::{
    manifest::AudioDisposition,
    reader::{IntegrityValidatedCartridge, ValidationOptions, open_integrity_validated},
};
use serde::Serialize;
use thiserror::Error;

use crate::codec_pack::{
    CodecPackError, ValidatedCodecPack, ValidatedExternalAsset, discover_codec_packs,
    validate_external_asset,
};

const H3_PACK_ID: &str = "org.latentdeck.h3";
const H3_ASSET_ID: &str = "taeh3";

/// Player lifecycle exposed to UI surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerPhase {
    Empty,
    Loading,
    Ready,
    Playing,
    Paused,
    Error,
}

/// External codec availability exposed without machine-local paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodecState {
    Missing,
    Loading,
    Ready,
    Incompatible,
    Error,
}

/// Fully validated cartridge metadata needed by the simple Player UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CartridgeSummary {
    pub cartridge_id: String,
    pub archive_sha256: String,
    pub file_name: String,
    pub width: u32,
    pub height: u32,
    pub frame_count: u64,
    pub frame_rate_numerator: u64,
    pub frame_rate_denominator: u64,
    pub audio_present: bool,
}

/// Codec state with a bounded, path-free operator hint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodecSummary {
    pub state: CodecState,
    pub display_name: Option<String>,
    pub detail: Option<String>,
    pub pack_id: Option<String>,
    pub pack_version: Option<String>,
    pub publisher_name: Option<String>,
    pub publisher_url: Option<String>,
    pub pack_license_label: Option<String>,
    pub decoder_asset_id: Option<String>,
    pub decoder_display_name: Option<String>,
    pub decoder_variants: Vec<DecoderVariantSummary>,
}

/// Path-free source, license, and integrity identity for one accepted weight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecoderVariantSummary {
    pub variant_id: String,
    pub sha256: String,
    pub byte_length: u64,
    pub source_url: String,
    pub license_label: String,
    pub license_url: String,
    pub selected: bool,
}

/// Stable user-facing failure state. Detailed diagnostics remain in logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerErrorView {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

/// Immutable UI snapshot. It contains no transport clock or mutable paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerView {
    pub revision: u64,
    pub phase: PlayerPhase,
    pub cartridge: Option<CartridgeSummary>,
    pub codec: CodecSummary,
    pub position_frame: u64,
    pub loop_enabled: bool,
    pub output_available: bool,
    pub error: Option<PlayerErrorView>,
}

/// Inputs retained only by trusted Core for worker launch and slot binding.
pub struct PlayerLaunchInputs<'a> {
    pub codec_pack: &'a ValidatedCodecPack,
    pub decoder_asset: &'a ValidatedExternalAsset,
    pub cartridge_path: &'a Path,
    pub cartridge: &'a CartridgeSummary,
}

/// Codec-neutral retained source identity used to construct a Protocol 2
/// session from an already integrity-validated read-only handle.
pub struct PlayerProtocol2SourceInputs<'a> {
    pub retained_cartridge: &'a IntegrityValidatedCartridge,
    pub cartridge: &'a CartridgeSummary,
}

struct LoadedCartridge {
    validated: IntegrityValidatedCartridge,
    path: PathBuf,
    summary: CartridgeSummary,
}

/// Synchronous selection state. Worker scheduling is layered on top of this
/// object and is never owned by the webview.
pub struct PlayerCoordinator {
    revision: u64,
    phase: PlayerPhase,
    packs: Vec<ValidatedCodecPack>,
    selected_pack: Option<usize>,
    codec_fault: Option<CodecSummary>,
    protocol2_codec: Option<CodecSummary>,
    decoder_asset: Option<ValidatedExternalAsset>,
    cartridge: Option<LoadedCartridge>,
    position_frame: u64,
    loop_enabled: bool,
    output_available: bool,
    error: Option<PlayerErrorView>,
}

impl PlayerCoordinator {
    /// Discover only the supplied exact Codec Pack roots.
    ///
    /// # Errors
    ///
    /// Returns a stable discovery error instead of silently ignoring a broken
    /// or conflicting installation.
    pub fn discover(roots: &[PathBuf], app_version: &str) -> Result<Self, PlayerCoordinatorError> {
        let packs =
            discover_codec_packs(roots, app_version).map_err(PlayerCoordinatorError::from)?;
        Ok(Self::with_validated_packs(packs))
    }

    /// Discover packs while retaining a visible failure state for desktop UI.
    #[must_use]
    pub fn discover_visible(roots: &[PathBuf], app_version: &str) -> Self {
        match Self::discover(roots, app_version) {
            Ok(player) => player,
            Err(error) => {
                let mut player = Self::without_codec();
                let state = if error.code.starts_with("codec.pack_incompatible_") {
                    CodecState::Incompatible
                } else {
                    CodecState::Error
                };
                player.codec_fault = Some(codec_summary_without_pack(
                    state,
                    Some(error.message.clone()),
                ));
                player.error = Some(PlayerErrorView {
                    code: error.code,
                    message: error.message,
                    recoverable: true,
                });
                player
            }
        }
    }

    /// Construct the visible missing-codec state without scanning the disk.
    #[must_use]
    pub fn without_codec() -> Self {
        Self::with_validated_packs(Vec::new())
    }

    fn with_validated_packs(packs: Vec<ValidatedCodecPack>) -> Self {
        Self {
            revision: 0,
            phase: PlayerPhase::Empty,
            packs,
            selected_pack: None,
            codec_fault: None,
            protocol2_codec: None,
            decoder_asset: None,
            cartridge: None,
            position_frame: 0,
            loop_enabled: false,
            output_available: false,
            error: None,
        }
    }

    /// Select one exact legacy H3 Codec Pack identity for the Player-only
    /// Protocol 1 bridge.
    ///
    /// Discovery deliberately never calls this method. Installed versions are
    /// immutable, side-by-side choices and the caller must supply both the
    /// package ID and version without a newest-version fallback.
    ///
    /// # Errors
    ///
    /// Returns a stable missing-package error when that exact validated H3
    /// identity is unavailable.
    pub fn select_codec_pack_exact(
        &mut self,
        pack_id: &str,
        pack_version: &str,
    ) -> Result<PlayerView, PlayerCoordinatorError> {
        let selected_pack = self
            .packs
            .iter()
            .position(|pack| {
                pack.manifest.pack_id == H3_PACK_ID
                    && pack.manifest.pack_id == pack_id
                    && pack.manifest.pack_version == pack_version
            })
            .ok_or_else(|| {
                PlayerCoordinatorError::new(
                    "codec.pack_missing",
                    "The exact selected H3 Codec Pack version is unavailable.",
                )
            })?;
        if self.selected_pack != Some(selected_pack) {
            self.decoder_asset = None;
        }
        self.selected_pack = Some(selected_pack);
        self.protocol2_codec = None;
        self.codec_fault = None;
        self.error = None;
        self.bump_revision()?;
        Ok(self.view())
    }

    /// Select and verify the external TAEH3 weight declared by the pack.
    ///
    /// # Errors
    ///
    /// Returns a stable error when no compatible pack is installed or the
    /// selected file is not an accepted exact variant.
    pub fn select_decoder_asset(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<PlayerView, PlayerCoordinatorError> {
        let pack = self
            .selected_codec_pack()
            .ok_or_else(|| self.codec_selection_error())?;
        let validation = validate_external_asset(pack, H3_ASSET_ID, path);
        match validation {
            Ok(asset) => {
                self.decoder_asset = Some(asset);
                self.codec_fault = None;
                self.error = None;
                self.bump_revision()?;
                Ok(self.view())
            }
            Err(error) => {
                let failure = PlayerCoordinatorError::from(error);
                let state = if failure.code == "codec.asset_incompatible" {
                    CodecState::Incompatible
                } else {
                    CodecState::Error
                };
                self.codec_fault = self.selected_codec_pack().map(|selected| {
                    codec_summary_for_pack(selected, state, Some(failure.message.clone()), None)
                });
                self.error = Some(PlayerErrorView {
                    code: failure.code.clone(),
                    message: failure.message.clone(),
                    recoverable: true,
                });
                self.bump_revision()?;
                Err(failure)
            }
        }
    }

    /// Fully validate and retain one `.lc` cartridge.
    ///
    /// # Errors
    ///
    /// Returns a stable LC error without exposing its absolute path.
    pub fn open_cartridge(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<PlayerView, PlayerCoordinatorError> {
        let path = path.as_ref();
        self.phase = PlayerPhase::Loading;
        let validated =
            open_integrity_validated(path, &ValidationOptions::default()).map_err(|error| {
                self.phase = PlayerPhase::Error;
                PlayerCoordinatorError::new(error.code(), error.detail)
            })?;
        let manifest = validated.manifest();
        let video = &manifest.timing.decoded_video;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("cartridge.lc")
            .to_owned();
        let summary = CartridgeSummary {
            cartridge_id: manifest.cartridge_id.0.clone(),
            archive_sha256: validated.receipt().archive_sha256.to_string(),
            file_name,
            width: video.width,
            height: video.height,
            frame_count: video.frame_count,
            frame_rate_numerator: video.frame_rate.numerator,
            frame_rate_denominator: video.frame_rate.denominator,
            audio_present: !matches!(manifest.audio, AudioDisposition::SourceAbsent),
        };
        self.cartridge = Some(LoadedCartridge {
            validated,
            path: path.to_path_buf(),
            summary,
        });
        self.phase = PlayerPhase::Ready;
        self.position_frame = 0;
        self.error = None;
        self.bump_revision()?;
        Ok(self.view())
    }

    /// Return worker inputs only after both trust decisions are complete.
    ///
    /// # Errors
    ///
    /// Returns a stable state error while the cartridge, pack, or explicit
    /// decoder asset is missing.
    pub fn launch_inputs(&self) -> Result<PlayerLaunchInputs<'_>, PlayerCoordinatorError> {
        let codec_pack = self
            .selected_codec_pack()
            .ok_or_else(|| self.codec_selection_error())?;
        let decoder_asset = self.decoder_asset.as_ref().ok_or_else(|| {
            PlayerCoordinatorError::new(
                "codec.asset_missing",
                "No compatible TAEH3 decoder weight is selected",
            )
        })?;
        let cartridge = self.cartridge.as_ref().ok_or_else(|| {
            PlayerCoordinatorError::new("slot.cartridge_missing", "No cartridge is loaded")
        })?;
        Ok(PlayerLaunchInputs {
            codec_pack,
            decoder_asset,
            cartridge_path: &cartridge.path,
            cartridge: &cartridge.summary,
        })
    }

    /// Borrow the already integrity-validated cartridge and its path-free UI
    /// summary for a fresh Protocol 2 session. Codec selection and asset trust
    /// remain owned by the common Extensions Manager.
    ///
    /// # Errors
    ///
    /// Returns while no cartridge is loaded.
    pub fn protocol2_source_inputs(
        &self,
    ) -> Result<PlayerProtocol2SourceInputs<'_>, PlayerCoordinatorError> {
        let cartridge = self.cartridge.as_ref().ok_or_else(|| {
            PlayerCoordinatorError::new("slot.cartridge_missing", "No cartridge is loaded")
        })?;
        Ok(PlayerProtocol2SourceInputs {
            retained_cartridge: &cartridge.validated,
            cartridge: &cartridge.summary,
        })
    }

    /// Replace the visible codec summary with one exact Protocol 2 package
    /// selection. The application owns the process-local package lease; Core
    /// never infers one from this path-free summary.
    ///
    /// # Errors
    ///
    /// Returns only if the monotonic UI revision is exhausted.
    pub fn set_protocol2_codec_summary(
        &mut self,
        summary: CodecSummary,
    ) -> Result<PlayerView, PlayerCoordinatorError> {
        self.protocol2_codec = Some(summary);
        self.codec_fault = None;
        self.error = None;
        self.bump_revision()?;
        Ok(self.view())
    }

    /// Record a Protocol 2 transport transition after exact package,
    /// cartridge, and profile negotiation succeeded.
    ///
    /// # Errors
    ///
    /// Returns when no cartridge is loaded, the requested transition is not
    /// valid from the current phase, or the monotonic revision is exhausted.
    pub fn set_playing_protocol2(
        &mut self,
        playing: bool,
    ) -> Result<PlayerView, PlayerCoordinatorError> {
        if self.cartridge.is_none() {
            return Err(PlayerCoordinatorError::new(
                "state.invalid_transition",
                "Load a cartridge before controlling playback.",
            ));
        }
        if playing {
            if !matches!(self.phase, PlayerPhase::Ready | PlayerPhase::Paused) {
                return Err(PlayerCoordinatorError::new(
                    "state.invalid_transition",
                    "Player is not ready to start.",
                ));
            }
            self.phase = PlayerPhase::Playing;
        } else if self.phase == PlayerPhase::Playing {
            self.phase = PlayerPhase::Paused;
        } else if self.phase != PlayerPhase::Paused {
            return Err(PlayerCoordinatorError::new(
                "state.invalid_transition",
                "Player is not currently playing.",
            ));
        }
        self.error = None;
        self.bump_revision()?;
        Ok(self.view())
    }

    /// Apply the post-reset P2 transport state without consulting legacy H3
    /// launch inputs.
    ///
    /// # Errors
    ///
    /// Returns when no cartridge is loaded or the monotonic revision is
    /// exhausted.
    pub fn reset_to_start_protocol2(&mut self) -> Result<PlayerView, PlayerCoordinatorError> {
        if self.cartridge.is_none() {
            return Err(PlayerCoordinatorError::new(
                "state.invalid_transition",
                "No cartridge is loaded.",
            ));
        }
        self.position_frame = 0;
        self.phase = PlayerPhase::Paused;
        self.error = None;
        self.bump_revision()?;
        Ok(self.view())
    }

    /// Update the transport loop policy without changing decoder state.
    ///
    /// # Errors
    ///
    /// Returns a state error when no cartridge is loaded.
    pub fn set_loop_enabled(
        &mut self,
        enabled: bool,
    ) -> Result<PlayerView, PlayerCoordinatorError> {
        if self.cartridge.is_none() {
            return Err(PlayerCoordinatorError::new(
                "state.invalid_transition",
                "Load a cartridge before changing Loop.",
            ));
        }
        self.loop_enabled = enabled;
        self.bump_revision()?;
        Ok(self.view())
    }

    /// Record a transport transition decided by trusted runtime scheduling.
    ///
    /// # Errors
    ///
    /// Returns a state error when launch inputs are incomplete or the current
    /// phase cannot perform the requested transition.
    pub fn set_playing(&mut self, playing: bool) -> Result<PlayerView, PlayerCoordinatorError> {
        if playing {
            self.launch_inputs()?;
            if !matches!(self.phase, PlayerPhase::Ready | PlayerPhase::Paused) {
                return Err(PlayerCoordinatorError::new(
                    "state.invalid_transition",
                    "Player is not ready to start.",
                ));
            }
            self.phase = PlayerPhase::Playing;
        } else if self.phase == PlayerPhase::Playing {
            self.phase = PlayerPhase::Paused;
        } else {
            return Err(PlayerCoordinatorError::new(
                "state.invalid_transition",
                "Player is not currently playing.",
            ));
        }
        self.error = None;
        self.bump_revision()?;
        Ok(self.view())
    }

    /// Apply the post-reset transport state after causal decoder reset.
    ///
    /// # Errors
    ///
    /// Returns a state error when launch inputs are incomplete.
    pub fn reset_to_start(&mut self) -> Result<PlayerView, PlayerCoordinatorError> {
        self.launch_inputs()?;
        self.position_frame = 0;
        self.phase = PlayerPhase::Paused;
        self.error = None;
        self.bump_revision()?;
        Ok(self.view())
    }

    /// Update the read-only frame progress reported by native presentation.
    ///
    /// # Errors
    ///
    /// Returns a state error for an absent cartridge or out-of-range frame.
    pub fn set_position_frame(
        &mut self,
        position_frame: u64,
    ) -> Result<PlayerView, PlayerCoordinatorError> {
        let frame_count = self
            .cartridge
            .as_ref()
            .map(|loaded| loaded.summary.frame_count)
            .ok_or_else(|| {
                PlayerCoordinatorError::new("state.invalid_transition", "No cartridge is loaded.")
            })?;
        if position_frame >= frame_count {
            return Err(PlayerCoordinatorError::new(
                "player.position_invalid",
                "Presented frame is outside the cartridge duration.",
            ));
        }
        self.position_frame = position_frame;
        self.bump_revision()?;
        Ok(self.view())
    }

    /// Make native output availability visible without exposing a window
    /// handle.
    ///
    /// # Errors
    ///
    /// Returns an error only if the monotonic view revision is exhausted.
    pub fn set_output_available(
        &mut self,
        available: bool,
    ) -> Result<PlayerView, PlayerCoordinatorError> {
        self.output_available = available;
        self.bump_revision()?;
        Ok(self.view())
    }

    /// Enter a safe error state after worker or renderer failure.
    ///
    /// # Errors
    ///
    /// Returns an error only if the monotonic view revision is exhausted.
    pub fn set_runtime_error(
        &mut self,
        code: impl Into<String>,
        message: impl Into<String>,
        recoverable: bool,
    ) -> Result<PlayerView, PlayerCoordinatorError> {
        self.phase = PlayerPhase::Error;
        self.output_available = false;
        self.error = Some(PlayerErrorView {
            code: code.into(),
            message: message.into(),
            recoverable,
        });
        self.bump_revision()?;
        Ok(self.view())
    }

    /// Current path-free snapshot.
    #[must_use]
    pub fn view(&self) -> PlayerView {
        PlayerView {
            revision: self.revision,
            phase: self.phase,
            cartridge: self.cartridge.as_ref().map(|loaded| loaded.summary.clone()),
            codec: self.codec_summary(),
            position_frame: self.position_frame,
            loop_enabled: self.loop_enabled,
            output_available: self.output_available,
            error: self.error.clone(),
        }
    }

    fn selected_codec_pack(&self) -> Option<&ValidatedCodecPack> {
        self.selected_pack.and_then(|index| self.packs.get(index))
    }

    fn codec_selection_error(&self) -> PlayerCoordinatorError {
        if self
            .packs
            .iter()
            .any(|pack| pack.manifest.pack_id == H3_PACK_ID)
        {
            PlayerCoordinatorError::new(
                "codec.selection_missing",
                "Select an exact compatible H3 Codec Pack version.",
            )
        } else {
            PlayerCoordinatorError::new(
                "codec.pack_missing",
                "Install a compatible H3 Codec Pack before selecting it.",
            )
        }
    }

    fn codec_summary(&self) -> CodecSummary {
        if let Some(summary) = &self.protocol2_codec {
            return summary.clone();
        }
        if let Some(fault) = &self.codec_fault {
            return fault.clone();
        }
        let Some(pack) = self.selected_codec_pack() else {
            let detail = if self
                .packs
                .iter()
                .any(|pack| pack.manifest.pack_id == H3_PACK_ID)
            {
                "Select an exact compatible H3 Codec Pack version."
            } else {
                "Install a compatible H3 Codec Pack."
            };
            return codec_summary_without_pack(CodecState::Missing, Some(detail.to_owned()));
        };
        if self.decoder_asset.is_none() {
            return codec_summary_for_pack(
                pack,
                CodecState::Missing,
                Some("Select a compatible TAEH3 decoder weight.".to_owned()),
                None,
            );
        }
        codec_summary_for_pack(pack, CodecState::Ready, None, self.decoder_asset.as_ref())
    }

    fn bump_revision(&mut self) -> Result<(), PlayerCoordinatorError> {
        self.revision = self.revision.checked_add(1).ok_or_else(|| {
            PlayerCoordinatorError::new("player.revision_exhausted", "Player revision exhausted")
        })?;
        Ok(())
    }
}

fn codec_summary_without_pack(state: CodecState, detail: Option<String>) -> CodecSummary {
    CodecSummary {
        state,
        display_name: None,
        detail,
        pack_id: None,
        pack_version: None,
        publisher_name: None,
        publisher_url: None,
        pack_license_label: None,
        decoder_asset_id: None,
        decoder_display_name: None,
        decoder_variants: Vec::new(),
    }
}

fn codec_summary_for_pack(
    pack: &ValidatedCodecPack,
    state: CodecState,
    detail: Option<String>,
    selected: Option<&ValidatedExternalAsset>,
) -> CodecSummary {
    let decoder = pack
        .manifest
        .external_assets
        .iter()
        .find(|asset| asset.asset_id == H3_ASSET_ID);
    CodecSummary {
        state,
        display_name: Some(pack.manifest.display_name.clone()),
        detail,
        pack_id: Some(pack.manifest.pack_id.clone()),
        pack_version: Some(pack.manifest.pack_version.clone()),
        publisher_name: Some(pack.manifest.publisher.name.clone()),
        publisher_url: pack.manifest.publisher.url.clone(),
        pack_license_label: Some(pack.manifest.license.spdx_or_label.clone()),
        decoder_asset_id: decoder.map(|asset| asset.asset_id.clone()),
        decoder_display_name: decoder.map(|asset| asset.display_name.clone()),
        decoder_variants: decoder
            .into_iter()
            .flat_map(|asset| asset.accepted_variants.iter())
            .map(|variant| DecoderVariantSummary {
                variant_id: variant.variant_id.clone(),
                sha256: variant.sha256.clone(),
                byte_length: variant.byte_length,
                source_url: variant.source_url.clone(),
                license_label: variant.license_label.clone(),
                license_url: variant.license_url.clone(),
                selected: selected.is_some_and(|selected| {
                    selected.asset_id == H3_ASSET_ID
                        && selected.variant_id == variant.variant_id
                        && selected.sha256 == variant.sha256
                        && selected.byte_length == variant.byte_length
                }),
            })
            .collect(),
    }
}

/// Stable command failure returned to the desktop shell.
#[derive(Debug, Error)]
#[error("{code}: {message}")]
pub struct PlayerCoordinatorError {
    pub code: String,
    pub message: String,
}

impl PlayerCoordinatorError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<CodecPackError> for PlayerCoordinatorError {
    fn from(error: CodecPackError) -> Self {
        Self::new(error.code, error.detail)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Cursor};

    use latentdeck_cartridge::{
        archive::{EntryWrite, payload_crc32, write_canonical},
        hash::hash_reader,
        limits::ValidationLimits,
        manifest::parse_manifest_json,
        writer::canonical_json_bytes,
    };

    use super::*;

    #[test]
    fn missing_codec_is_visible_without_scanning_or_fake_playability() {
        let player = PlayerCoordinator::without_codec();
        let view = player.view();

        assert_eq!(view.phase, PlayerPhase::Empty);
        assert_eq!(view.codec.state, CodecState::Missing);
        assert!(player.launch_inputs().is_err());
    }

    #[test]
    fn opening_a_cartridge_runs_full_validation_and_exposes_no_path() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("synthetic.lc");
        fs::write(&path, synthetic_lc()).expect("synthetic cartridge");
        let mut player = PlayerCoordinator::without_codec();

        let view = player.open_cartridge(&path).expect("validated cartridge");

        assert_eq!(view.phase, PlayerPhase::Ready);
        let cartridge = view.cartridge.expect("summary");
        assert_eq!(cartridge.file_name, "synthetic.lc");
        assert_eq!(cartridge.frame_count, 5);
        assert_eq!(cartridge.width, 16);
        assert!(!cartridge.audio_present);
        let encoded = serde_json::to_string(&player.view()).expect("serialize view");
        assert!(!encoded.contains(directory.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn protocol2_source_reuses_the_retained_integrity_handle() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("synthetic.lc");
        fs::write(&path, synthetic_lc()).expect("synthetic cartridge");
        let mut player = PlayerCoordinator::without_codec();
        player.open_cartridge(&path).expect("validated cartridge");

        let source = player
            .protocol2_source_inputs()
            .expect("Protocol 2 retained source");
        let retained = source
            .retained_cartridge
            .try_clone_retained()
            .expect("duplicate retained handle without reopening the path");

        assert_eq!(
            retained.receipt().archive_sha256.to_string(),
            source.cartridge.archive_sha256
        );
        assert_eq!(
            retained.manifest().cartridge_id.0,
            source.cartridge.cartridge_id
        );
    }

    #[test]
    fn corrupt_cartridge_never_replaces_the_retained_valid_slot() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let valid = directory.path().join("valid.lc");
        let corrupt = directory.path().join("corrupt.lc");
        fs::write(&valid, synthetic_lc()).expect("valid cartridge");
        fs::write(&corrupt, b"not an LC archive").expect("corrupt cartridge");
        let mut player = PlayerCoordinator::without_codec();
        player.open_cartridge(&valid).expect("initial slot");

        let error = player
            .open_cartridge(&corrupt)
            .expect_err("corrupt replacement");

        assert!(!error.code.is_empty());
        assert_eq!(
            player.view().cartridge.expect("retained slot").file_name,
            "valid.lc"
        );
    }

    fn synthetic_lc() -> Vec<u8> {
        let tensor_bytes = vec![0_u8; 24 * 2 * 2];
        let mut header = format!(
            r#"{{"video":{{"data_offsets":[0,{}],"dtype":"F16","shape":[1,24,2,1,1]}}}}"#,
            tensor_bytes.len()
        )
        .into_bytes();
        while !header.len().is_multiple_of(8) {
            header.push(b' ');
        }
        let mut payload = Vec::new();
        payload.extend_from_slice(&(header.len() as u64).to_le_bytes());
        payload.extend_from_slice(&header);
        payload.extend_from_slice(&tensor_bytes);
        let measured = hash_reader(&mut Cursor::new(&payload)).expect("payload hash");
        let manifest_value = serde_json::json!({
            "spec_version": "0.1.0",
            "cartridge_id": "550e8400-e29b-41d4-a716-446655440000",
            "codec": {"family": "minimax_h3", "profile": "h3_av_latent", "profile_version": "0.1.0"},
            "payloads": [{
                "path": "payloads/h3.safetensors",
                "media_type": "application/vnd.safetensors",
                "byte_length": measured.byte_length,
                "sha256": measured.sha256.to_string()
            }],
            "tensors": [{
                "stream": "visual", "name": "video", "payload": "payloads/h3.safetensors",
                "storage_dtype": "F16", "runtime_dtype": "F16", "shape": [1,24,2,1,1]
            }],
            "timing": {
                "contract": "minimax_h3_causal", "contract_version": "0.1.0",
                "decoded_video": {
                    "width": 16, "height": 16, "frame_count": 5,
                    "frame_rate": {"numerator": 24, "denominator": 1},
                    "duration": {"numerator": 5, "denominator": 24}
                }
            },
            "audio": {"policy": "source_absent"},
            "provenance": {"created_by": {"name": "latentdeck-test", "version": "0.1.0"}, "sources": []},
            "parent_cartridges": [], "operation_history": []
        });
        let parsed = parse_manifest_json(
            &serde_json::to_vec(&manifest_value).expect("manifest JSON"),
            &ValidationLimits::default(),
        )
        .expect("manifest");
        let manifest = canonical_json_bytes(&parsed).expect("canonical manifest");
        let mut manifest_reader = Cursor::new(&manifest);
        let mut payload_reader = Cursor::new(&payload);
        let mut entries = [
            EntryWrite::new(
                "manifest.json",
                manifest.len() as u64,
                payload_crc32(&manifest),
                &mut manifest_reader,
            ),
            EntryWrite::new(
                "payloads/h3.safetensors",
                payload.len() as u64,
                payload_crc32(&payload),
                &mut payload_reader,
            ),
        ];
        let mut archive = Cursor::new(Vec::new());
        write_canonical(&mut archive, &mut entries).expect("LC archive");
        archive.into_inner()
    }
}
