//! Player-specific identity and embedded-viewport adapter over native output.

use std::sync::{Mutex, MutexGuard};

use latentdeck_native_output::NativeOutputConfig;
pub use latentdeck_native_output::{NativeOutput, NativeOutputBounds, ResizeOutcome};
use serde::{Deserialize, Serialize};

/// Stable Tauri label for the Player's decoded-frame output window.
pub const NATIVE_OUTPUT_WINDOW_LABEL: &str = "latentplayer-native-output";

const NATIVE_OUTPUT_WINDOW_TITLE: &str = "LatentPlayer Output";

const MIN_SCALE_FACTOR: f64 = 0.5;
const MAX_SCALE_FACTOR: f64 = 8.0;
const SCALE_FACTOR_TOLERANCE: f64 = 0.01;
const CLIENT_ROUNDING_TOLERANCE: i64 = 2;

/// Revisioned CSS viewport measurement emitted by the trusted Player UI.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewportBoundsRequest {
    epoch: u64,
    revision: u64,
    x_css: f64,
    y_css: f64,
    width_css: f64,
    height_css: f64,
    scale_factor: f64,
    visible: bool,
}

/// Host-issued identity for one mounted Player `WebView` viewport client.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewportSessionAck {
    epoch: u64,
}

/// Latest validated child-window placement owned by the Rust application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlayerViewport {
    revision: u64,
    bounds: Option<NativeOutputBounds>,
    visible: bool,
}

impl PlayerViewport {
    #[must_use]
    pub const fn revision(self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn bounds(self) -> Option<NativeOutputBounds> {
        self.bounds
    }

    #[must_use]
    pub const fn visible(self) -> bool {
        self.visible
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewportGeometry {
    bounds: Option<NativeOutputBounds>,
    visible: bool,
    x_css: f64,
    y_css: f64,
    width_css: f64,
    height_css: f64,
    scale_factor: f64,
}

impl ViewportGeometry {
    const fn hidden() -> Self {
        Self {
            bounds: None,
            visible: false,
            x_css: 0.0,
            y_css: 0.0,
            width_css: 0.0,
            height_css: 0.0,
            scale_factor: 1.0,
        }
    }

    const fn viewport(self, host_revision: u64) -> PlayerViewport {
        PlayerViewport {
            revision: host_revision,
            bounds: self.bounds,
            visible: self.visible,
        }
    }
}

/// Fully validated client request awaiting host-authoritative ordering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValidatedViewportRequest {
    epoch: u64,
    client_revision: u64,
    geometry: ViewportGeometry,
}

/// Stable validation categories for the WebView-to-child placement boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewportBoundsError {
    NonFinite,
    InvalidScaleFactor,
    StaleScaleFactor,
    InvalidExtent,
    OutsideClient,
    Overflow,
}

fn validate_scale_factors(requested: f64, authoritative: f64) -> Result<(), ViewportBoundsError> {
    if !(MIN_SCALE_FACTOR..=MAX_SCALE_FACTOR).contains(&requested)
        || !(MIN_SCALE_FACTOR..=MAX_SCALE_FACTOR).contains(&authoritative)
    {
        return Err(ViewportBoundsError::InvalidScaleFactor);
    }
    if (requested - authoritative).abs() > SCALE_FACTOR_TOLERANCE {
        return Err(ViewportBoundsError::StaleScaleFactor);
    }
    Ok(())
}

/// Convert CSS edges to a conservative physical rectangle using the
/// authoritative current Tauri scale factor.
///
/// Left/top use floor and right/bottom use ceil so fractional-DPI rounding
/// cannot expose a `WebView` seam. A two-pixel client-edge tolerance is clamped
/// for normal browser/OS rounding differences at fractional DPI scales.
pub fn validate_viewport_bounds(
    request: ViewportBoundsRequest,
    authoritative_scale_factor: f64,
    client_width: u32,
    client_height: u32,
) -> Result<ValidatedViewportRequest, ViewportBoundsError> {
    let values = [
        request.x_css,
        request.y_css,
        request.width_css,
        request.height_css,
        request.scale_factor,
        authoritative_scale_factor,
    ];
    if values.iter().any(|value| !value.is_finite()) {
        return Err(ViewportBoundsError::NonFinite);
    }
    validate_scale_factors(request.scale_factor, authoritative_scale_factor)?;
    if request.x_css < 0.0
        || request.y_css < 0.0
        || request.width_css < 0.0
        || request.height_css < 0.0
    {
        return Err(ViewportBoundsError::InvalidExtent);
    }
    if request.width_css == 0.0 || request.height_css == 0.0 {
        if request.visible {
            return Err(ViewportBoundsError::InvalidExtent);
        }
        return Ok(ValidatedViewportRequest {
            epoch: request.epoch,
            client_revision: request.revision,
            geometry: ViewportGeometry {
                bounds: None,
                visible: false,
                x_css: request.x_css,
                y_css: request.y_css,
                width_css: request.width_css,
                height_css: request.height_css,
                scale_factor: request.scale_factor,
            },
        });
    }
    if client_width == 0 || client_height == 0 {
        return Err(ViewportBoundsError::InvalidExtent);
    }

    let left = (request.x_css * authoritative_scale_factor).floor();
    let top = (request.y_css * authoritative_scale_factor).floor();
    let right = ((request.x_css + request.width_css) * authoritative_scale_factor).ceil();
    let bottom = ((request.y_css + request.height_css) * authoritative_scale_factor).ceil();
    if [left, top, right, bottom]
        .iter()
        .any(|value| *value < 0.0 || *value > f64::from(u32::MAX))
    {
        return Err(ViewportBoundsError::Overflow);
    }

    #[allow(clippy::cast_possible_truncation)] // Range and finiteness checked above.
    let left = left as i64;
    #[allow(clippy::cast_possible_truncation)] // Range and finiteness checked above.
    let top = top as i64;
    #[allow(clippy::cast_possible_truncation)] // Range and finiteness checked above.
    let mut right = right as i64;
    #[allow(clippy::cast_possible_truncation)] // Range and finiteness checked above.
    let mut bottom = bottom as i64;
    let client_right = i64::from(client_width);
    let client_bottom = i64::from(client_height);
    if right > client_right + CLIENT_ROUNDING_TOLERANCE
        || bottom > client_bottom + CLIENT_ROUNDING_TOLERANCE
        || left >= client_right
        || top >= client_bottom
    {
        return Err(ViewportBoundsError::OutsideClient);
    }
    right = right.min(client_right);
    bottom = bottom.min(client_bottom);
    let width = right
        .checked_sub(left)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(ViewportBoundsError::Overflow)?;
    let height = bottom
        .checked_sub(top)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(ViewportBoundsError::Overflow)?;
    let x = i32::try_from(left).map_err(|_| ViewportBoundsError::Overflow)?;
    let y = i32::try_from(top).map_err(|_| ViewportBoundsError::Overflow)?;
    let bounds = NativeOutputBounds::new(x, y, width, height)
        .map_err(|_| ViewportBoundsError::InvalidExtent)?;
    Ok(ValidatedViewportRequest {
        epoch: request.epoch,
        client_revision: request.revision,
        geometry: ViewportGeometry {
            bounds: Some(bounds),
            visible: request.visible,
            x_css: request.x_css,
            y_css: request.y_css,
            width_css: request.width_css,
            height_css: request.height_css,
            scale_factor: request.scale_factor,
        },
    })
}

#[derive(Debug)]
struct ViewportStoreState {
    epoch: u64,
    client_revision: u64,
    host_revision: u64,
    geometry: Option<ViewportGeometry>,
    viewport: Option<PlayerViewport>,
}

/// Host-authoritative viewport ordering that survives `WebView` reloads.
#[derive(Debug)]
pub struct PlayerViewportStore {
    state: Mutex<ViewportStoreState>,
}

impl PlayerViewportStore {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(ViewportStoreState {
                epoch: 0,
                client_revision: 0,
                host_revision: 0,
                geometry: None,
                viewport: None,
            }),
        }
    }

    /// Begin a new `WebView` measurement session and hide any prior child until
    /// the new client supplies a validated visible anchor.
    pub fn begin_session(
        &self,
    ) -> Result<(ViewportSessionAck, PlayerViewport), ViewportStoreError> {
        let mut state = self.lock()?;
        state.epoch = state
            .epoch
            .checked_add(1)
            .ok_or(ViewportStoreError::EpochOverflow)?;
        state.host_revision = state
            .host_revision
            .checked_add(1)
            .ok_or(ViewportStoreError::HostRevisionOverflow)?;
        state.client_revision = 0;
        let geometry = ViewportGeometry::hidden();
        let viewport = geometry.viewport(state.host_revision);
        state.geometry = Some(geometry);
        state.viewport = Some(viewport);
        Ok((ViewportSessionAck { epoch: state.epoch }, viewport))
    }

    /// Apply a request only inside the current epoch. A same-revision retry is
    /// accepted only when every validated client measurement is identical.
    pub fn apply(
        &self,
        request: ValidatedViewportRequest,
    ) -> Result<PlayerViewport, ViewportStoreError> {
        let mut state = self.lock()?;
        if request.epoch == 0 || request.epoch != state.epoch {
            return Err(ViewportStoreError::SessionStale);
        }
        if request.client_revision == 0 {
            return Err(ViewportStoreError::RevisionInvalid);
        }
        if request.client_revision < state.client_revision {
            return Err(ViewportStoreError::RevisionStale);
        }
        if request.client_revision == state.client_revision {
            if state.geometry != Some(request.geometry) {
                return Err(ViewportStoreError::RevisionConflict);
            }
            return state.viewport.ok_or(ViewportStoreError::NotReady);
        }

        state.host_revision = state
            .host_revision
            .checked_add(1)
            .ok_or(ViewportStoreError::HostRevisionOverflow)?;
        state.client_revision = request.client_revision;
        let viewport = request.geometry.viewport(state.host_revision);
        state.geometry = Some(request.geometry);
        state.viewport = Some(viewport);
        Ok(viewport)
    }

    /// Confirm that an awaited actor placement still represents the latest
    /// host state rather than a superseded same-channel acknowledgement.
    pub fn confirm_applied(
        &self,
        request: ValidatedViewportRequest,
        viewport: PlayerViewport,
    ) -> Result<(), ViewportStoreError> {
        let state = self.lock()?;
        if request.epoch != state.epoch {
            return Err(ViewportStoreError::SessionStale);
        }
        if request.client_revision != state.client_revision || state.viewport != Some(viewport) {
            return Err(ViewportStoreError::RevisionStale);
        }
        Ok(())
    }

    pub fn confirm_session(
        &self,
        session: ViewportSessionAck,
        viewport: PlayerViewport,
    ) -> Result<(), ViewportStoreError> {
        let state = self.lock()?;
        if session.epoch != state.epoch {
            return Err(ViewportStoreError::SessionStale);
        }
        if state.client_revision != 0 || state.viewport != Some(viewport) {
            return Err(ViewportStoreError::RevisionStale);
        }
        Ok(())
    }

    pub fn current(&self) -> Result<PlayerViewport, ViewportStoreError> {
        self.lock()?.viewport.ok_or(ViewportStoreError::NotReady)
    }

    pub fn current_visible(&self) -> Result<PlayerViewport, ViewportStoreError> {
        let viewport = self.current()?;
        if viewport.bounds().is_none() || !viewport.visible() {
            return Err(ViewportStoreError::NotVisible);
        }
        Ok(viewport)
    }

    fn lock(&self) -> Result<MutexGuard<'_, ViewportStoreState>, ViewportStoreError> {
        self.state
            .lock()
            .map_err(|_| ViewportStoreError::StateUnavailable)
    }
}

impl Default for PlayerViewportStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable failures for the host-issued viewport session boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewportStoreError {
    StateUnavailable,
    SessionStale,
    RevisionInvalid,
    RevisionStale,
    RevisionConflict,
    NotReady,
    NotVisible,
    EpochOverflow,
    HostRevisionOverflow,
}

impl ViewportStoreError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::StateUnavailable => "output.viewport_state_unavailable",
            Self::SessionStale => "output.viewport_session_stale",
            Self::RevisionInvalid => "output.viewport_revision_invalid",
            Self::RevisionStale => "output.viewport_revision_stale",
            Self::RevisionConflict => "output.viewport_revision_conflict",
            Self::NotReady | Self::NotVisible => "output.viewport_not_ready",
            Self::EpochOverflow => "output.viewport_epoch_overflow",
            Self::HostRevisionOverflow => "output.viewport_revision_overflow",
        }
    }

    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::StateUnavailable => {
                "The embedded video-area state is unavailable; restart LatentPlayer."
            }
            Self::SessionStale => {
                "The embedded video-area session is stale; begin a new viewport session."
            }
            Self::RevisionInvalid => "Embedded video-area revisions must start at one.",
            Self::RevisionStale => {
                "LatentPlayer rejected an out-of-order embedded video-area update."
            }
            Self::RevisionConflict => {
                "One embedded video-area revision cannot describe two geometries."
            }
            Self::NotReady => "LatentPlayer is waiting for the embedded video area.",
            Self::NotVisible => {
                "The embedded video area is hidden; restore the app window and try Play again."
            }
            Self::EpochOverflow => "The embedded video-area session counter is exhausted.",
            Self::HostRevisionOverflow => {
                "The embedded video-area host revision counter is exhausted."
            }
        }
    }
}

pub fn native_output_config(frame_width: u32, frame_height: u32) -> NativeOutputConfig {
    NativeOutputConfig::new(
        frame_width,
        frame_height,
        NATIVE_OUTPUT_WINDOW_LABEL,
        NATIVE_OUTPUT_WINDOW_TITLE,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_window_identity_is_preserved() {
        let config = native_output_config(800, 448);
        assert_eq!(config.frame_width, 800);
        assert_eq!(config.frame_height, 448);
        assert_eq!(config.window_label(), "latentplayer-native-output");
        assert_eq!(config.window_title(), "LatentPlayer Output");
    }

    fn request(scale_factor: f64) -> ViewportBoundsRequest {
        ViewportBoundsRequest {
            epoch: 1,
            revision: 7,
            x_css: 10.25,
            y_css: 20.25,
            width_css: 400.5,
            height_css: 300.5,
            scale_factor,
            visible: true,
        }
    }

    #[test]
    fn css_edges_convert_at_common_windows_dpi_scales() {
        let cases = [
            (1.0, 10, 20, 401, 301),
            (1.25, 12, 25, 502, 376),
            (1.5, 15, 30, 602, 452),
            (2.0, 20, 40, 802, 602),
        ];
        for (scale, expected_x, expected_y, expected_width, expected_height) in cases {
            let viewport = validate_viewport_bounds(request(scale), scale, 2_000, 2_000)
                .expect("valid viewport");
            let bounds = viewport.geometry.bounds.expect("physical bounds");
            assert_eq!(bounds.x(), expected_x);
            assert_eq!(bounds.y(), expected_y);
            assert_eq!(bounds.width(), expected_width);
            assert_eq!(bounds.height(), expected_height);
            assert!(viewport.geometry.visible);
        }
    }

    #[test]
    fn invalid_or_out_of_client_measurements_are_rejected() {
        let mut invalid = request(1.0);
        invalid.width_css = f64::NAN;
        assert_eq!(
            validate_viewport_bounds(invalid, 1.0, 960, 600),
            Err(ViewportBoundsError::NonFinite)
        );

        let mut outside = request(1.0);
        outside.x_css = 900.0;
        outside.width_css = 100.0;
        assert_eq!(
            validate_viewport_bounds(outside, 1.0, 960, 600),
            Err(ViewportBoundsError::OutsideClient)
        );
    }

    #[test]
    fn fractional_dpi_fullscreen_edge_noise_is_clamped_to_the_client() {
        let fullscreen = ViewportBoundsRequest {
            epoch: 1,
            revision: 9,
            x_css: 0.0,
            y_css: 0.0,
            width_css: 1_707.333_374_023_437_5,
            height_css: 960.0,
            scale_factor: 1.5,
            visible: true,
        };

        let viewport = validate_viewport_bounds(fullscreen, 1.5, 2_560, 1_440)
            .expect("fractional fullscreen bounds");
        let bounds = viewport.geometry.bounds.expect("visible fullscreen bounds");
        assert_eq!(bounds.x(), 0);
        assert_eq!(bounds.y(), 0);
        assert_eq!(bounds.width(), 2_560);
        assert_eq!(bounds.height(), 1_440);
    }

    #[test]
    fn hidden_zero_extent_suspends_without_inventing_physical_bounds() {
        let mut hidden = request(1.0);
        hidden.width_css = 0.0;
        hidden.visible = false;

        let viewport = validate_viewport_bounds(hidden, 1.0, 960, 600)
            .expect("hidden viewport is representable");
        assert_eq!(viewport.geometry.bounds, None);
        assert!(!viewport.geometry.visible);
    }

    #[test]
    fn stale_webview_scale_is_rejected_before_coordinate_conversion() {
        assert_eq!(
            validate_viewport_bounds(request(1.25), 1.5, 2_000, 2_000),
            Err(ViewportBoundsError::StaleScaleFactor)
        );
        assert!(validate_viewport_bounds(request(1.495), 1.5, 2_000, 2_000).is_ok());
    }

    #[test]
    fn host_epoch_and_revision_order_are_monotonic_and_idempotent() {
        let store = PlayerViewportStore::new();
        let (session, hidden) = store.begin_session().expect("first session");
        assert_eq!(session.epoch, 1);
        assert_eq!(hidden.revision(), 1);
        assert!(!hidden.visible());

        let first_request =
            validate_viewport_bounds(request(1.0), 1.0, 960, 600).expect("validated request");
        let visible = store.apply(first_request).expect("first apply");
        assert_eq!(visible.revision(), 2);
        assert!(visible.visible());
        assert_eq!(store.apply(first_request).expect("exact retry"), visible);
        store
            .confirm_applied(first_request, visible)
            .expect("latest placement confirmation");

        let mut conflict = first_request;
        conflict.geometry.width_css += 1.0;
        assert_eq!(
            store.apply(conflict),
            Err(ViewportStoreError::RevisionConflict)
        );

        let mut newer_wire = request(1.0);
        newer_wire.revision = 8;
        newer_wire.x_css = 12.25;
        let newer_request =
            validate_viewport_bounds(newer_wire, 1.0, 960, 600).expect("newer validated request");
        let newer_viewport = store.apply(newer_request).expect("newer apply");
        assert!(newer_viewport.revision() > visible.revision());
        assert_eq!(
            store.confirm_applied(first_request, visible),
            Err(ViewportStoreError::RevisionStale)
        );

        let (second, reloaded_hidden) = store.begin_session().expect("second session");
        assert_eq!(second.epoch, 2);
        assert!(reloaded_hidden.revision() > visible.revision());
        store
            .confirm_session(second, reloaded_hidden)
            .expect("latest session confirmation");
        assert_eq!(
            store.confirm_applied(first_request, visible),
            Err(ViewportStoreError::SessionStale)
        );
        assert_eq!(
            store.apply(first_request),
            Err(ViewportStoreError::SessionStale)
        );
    }
}
