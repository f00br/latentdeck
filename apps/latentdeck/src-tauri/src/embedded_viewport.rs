//! Shared trusted WebView-to-native-child placement contract for all Decks.

use std::sync::Mutex;

use latentdeck_native_output::NativeOutputBounds;
use serde::{Deserialize, Serialize};

use crate::library_state::CommandError;

const MIN_SCALE_FACTOR: f64 = 0.5;
const MAX_SCALE_FACTOR: f64 = 8.0;
const SCALE_FACTOR_MATCH_TOLERANCE: f64 = 0.01;
const CLIENT_ROUNDING_TOLERANCE: i64 = 2;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ViewportBoundsRequest {
    epoch: u64,
    revision: u64,
    x_css: f64,
    y_css: f64,
    width_css: f64,
    height_css: f64,
    scale_factor: f64,
    visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ViewportSessionAck {
    epoch: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EmbeddedViewport {
    // Host-applied revisions never reset when a WebView session begins, so
    // actor-side ordering remains valid across reloads.
    revision: u64,
    bounds: Option<NativeOutputBounds>,
    visible: bool,
}

impl EmbeddedViewport {
    #[must_use]
    pub(crate) const fn revision(self) -> u64 {
        self.revision
    }

    #[must_use]
    pub(crate) const fn bounds(self) -> Option<NativeOutputBounds> {
        self.bounds
    }

    #[must_use]
    pub(crate) const fn visible(self) -> bool {
        self.visible
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewportGeometry {
    bounds: Option<NativeOutputBounds>,
    visible: bool,
    // Retain the exact validated client measurement so a repeated client
    // revision cannot silently describe a different request that rounds to the
    // same physical rectangle.
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

    const fn viewport(self, host_revision: u64) -> EmbeddedViewport {
        EmbeddedViewport {
            revision: host_revision,
            bounds: self.bounds,
            visible: self.visible,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ValidatedViewportRequest {
    epoch: u64,
    client_revision: u64,
    geometry: ViewportGeometry,
}

impl ValidatedViewportRequest {
    #[cfg(test)]
    const fn bounds(self) -> Option<NativeOutputBounds> {
        self.geometry.bounds
    }

    #[cfg(test)]
    const fn visible(self) -> bool {
        self.geometry.visible
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewportBoundsError {
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
    if (requested - authoritative).abs() > SCALE_FACTOR_MATCH_TOLERANCE {
        return Err(ViewportBoundsError::StaleScaleFactor);
    }
    Ok(())
}

pub(crate) fn validate_viewport_bounds(
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

    #[allow(clippy::cast_possible_truncation)]
    let left = left as i64;
    #[allow(clippy::cast_possible_truncation)]
    let top = top as i64;
    #[allow(clippy::cast_possible_truncation)]
    let mut right = right as i64;
    #[allow(clippy::cast_possible_truncation)]
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

pub(crate) fn viewport_error(error: ViewportBoundsError) -> CommandError {
    let code = match error {
        ViewportBoundsError::NonFinite => "output.viewport_non_finite",
        ViewportBoundsError::InvalidScaleFactor => "output.viewport_scale_invalid",
        ViewportBoundsError::StaleScaleFactor => "output.viewport_scale_stale",
        ViewportBoundsError::InvalidExtent => "output.viewport_extent_invalid",
        ViewportBoundsError::OutsideClient => "output.viewport_outside_client",
        ViewportBoundsError::Overflow => "output.viewport_overflow",
    };
    CommandError::new(
        code,
        "LatentDeck rejected an invalid embedded video-area measurement.",
    )
}

#[derive(Debug)]
struct ViewportStoreState {
    epoch: u64,
    client_revision: u64,
    host_revision: u64,
    geometry: Option<ViewportGeometry>,
    viewport: Option<EmbeddedViewport>,
}

/// Host-authoritative viewport ordering shared by all first-party Decks.
///
/// Client revisions restart inside a newly issued epoch. Host revisions never
/// restart, which makes delayed commands and `WebView` reloads safe for a live
/// runtime actor.
#[derive(Debug)]
pub(crate) struct EmbeddedViewportStore {
    state: Mutex<ViewportStoreState>,
}

impl EmbeddedViewportStore {
    pub(crate) const fn new() -> Self {
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

    pub(crate) fn begin_session(
        &self,
    ) -> Result<(ViewportSessionAck, EmbeddedViewport), CommandError> {
        let mut state = self.lock()?;
        state.epoch = state.epoch.checked_add(1).ok_or_else(epoch_overflow)?;
        state.host_revision = state
            .host_revision
            .checked_add(1)
            .ok_or_else(host_revision_overflow)?;
        state.client_revision = 0;
        let geometry = ViewportGeometry::hidden();
        let viewport = geometry.viewport(state.host_revision);
        state.geometry = Some(geometry);
        state.viewport = Some(viewport);
        Ok((ViewportSessionAck { epoch: state.epoch }, viewport))
    }

    pub(crate) fn apply(
        &self,
        request: ValidatedViewportRequest,
    ) -> Result<EmbeddedViewport, CommandError> {
        let mut state = self.lock()?;
        if request.epoch == 0 || request.epoch != state.epoch {
            return Err(stale_session());
        }
        if request.client_revision == 0 {
            return Err(CommandError::new(
                "output.viewport_revision_invalid",
                "Embedded video-area revisions must start at one.",
            ));
        }
        if request.client_revision < state.client_revision {
            return Err(stale_revision());
        }
        if request.client_revision == state.client_revision {
            if state.geometry != Some(request.geometry) {
                return Err(CommandError::new(
                    "output.viewport_revision_conflict",
                    "One embedded video-area revision cannot describe two geometries.",
                ));
            }
            return state.viewport.ok_or_else(viewport_not_ready);
        }

        state.host_revision = state
            .host_revision
            .checked_add(1)
            .ok_or_else(host_revision_overflow)?;
        state.client_revision = request.client_revision;
        let viewport = request.geometry.viewport(state.host_revision);
        state.geometry = Some(request.geometry);
        state.viewport = Some(viewport);
        Ok(viewport)
    }

    /// Confirm that an awaited actor placement still belongs to the latest
    /// host state. Without this second check, a superseded command could
    /// receive an `Unchanged` actor reply and falsely acknowledge placement.
    pub(crate) fn confirm_applied(
        &self,
        request: ValidatedViewportRequest,
        viewport: EmbeddedViewport,
    ) -> Result<(), CommandError> {
        let state = self.lock()?;
        if request.epoch != state.epoch {
            return Err(stale_session());
        }
        if request.client_revision != state.client_revision || state.viewport != Some(viewport) {
            return Err(stale_revision());
        }
        Ok(())
    }

    pub(crate) fn confirm_session(
        &self,
        session: ViewportSessionAck,
        viewport: EmbeddedViewport,
    ) -> Result<(), CommandError> {
        let state = self.lock()?;
        if session.epoch != state.epoch {
            return Err(stale_session());
        }
        if state.client_revision != 0 || state.viewport != Some(viewport) {
            return Err(stale_revision());
        }
        Ok(())
    }

    pub(crate) fn current(&self) -> Result<EmbeddedViewport, CommandError> {
        self.lock()?.viewport.ok_or_else(viewport_not_ready)
    }

    pub(crate) fn current_visible(&self) -> Result<EmbeddedViewport, CommandError> {
        let viewport = self.current()?;
        if viewport.bounds().is_none() || !viewport.visible() {
            return Err(CommandError::new(
                "output.viewport_not_ready",
                "LatentDeck is waiting for a visible embedded video area.",
            ));
        }
        Ok(viewport)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ViewportStoreState>, CommandError> {
        self.state.lock().map_err(|_| {
            CommandError::new(
                "output.viewport_state_unavailable",
                "The embedded video-area state is unavailable.",
            )
        })
    }
}

fn viewport_not_ready() -> CommandError {
    CommandError::new(
        "output.viewport_not_ready",
        "LatentDeck is waiting for the embedded video area.",
    )
}

fn stale_session() -> CommandError {
    CommandError::new(
        "output.viewport_session_stale",
        "The embedded video-area session is stale; begin a new viewport session.",
    )
}

fn stale_revision() -> CommandError {
    CommandError::new(
        "output.viewport_revision_stale",
        "LatentDeck rejected an out-of-order embedded video-area update.",
    )
}

fn epoch_overflow() -> CommandError {
    CommandError::new(
        "output.viewport_epoch_overflow",
        "The embedded video-area session counter is exhausted.",
    )
}

fn host_revision_overflow() -> CommandError {
    CommandError::new(
        "output.viewport_revision_overflow",
        "The embedded video-area host revision counter is exhausted.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(epoch: u64, revision: u64, scale_factor: f64) -> ViewportBoundsRequest {
        ViewportBoundsRequest {
            epoch,
            revision,
            x_css: 10.25,
            y_css: 20.25,
            width_css: 400.5,
            height_css: 300.5,
            scale_factor,
            visible: true,
        }
    }

    fn validated(epoch: u64, revision: u64, scale_factor: f64) -> ValidatedViewportRequest {
        validate_viewport_bounds(
            request(epoch, revision, scale_factor),
            scale_factor,
            2_000,
            2_000,
        )
        .expect("valid viewport")
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
            let request = validated(1, 7, scale);
            let bounds = request.bounds().expect("physical bounds");
            assert_eq!(bounds.x(), expected_x);
            assert_eq!(bounds.y(), expected_y);
            assert_eq!(bounds.width(), expected_width);
            assert_eq!(bounds.height(), expected_height);
            assert!(request.visible());
        }
    }

    #[test]
    fn invalid_or_out_of_client_measurements_are_rejected() {
        let mut invalid = request(1, 1, 1.0);
        invalid.width_css = f64::NAN;
        assert_eq!(
            validate_viewport_bounds(invalid, 1.0, 960, 600),
            Err(ViewportBoundsError::NonFinite)
        );

        let mut outside = request(1, 1, 1.0);
        outside.x_css = 900.0;
        outside.width_css = 100.0;
        assert_eq!(
            validate_viewport_bounds(outside, 1.0, 960, 600),
            Err(ViewportBoundsError::OutsideClient)
        );

        let stale_scale = request(1, 1, 1.0);
        assert_eq!(
            validate_viewport_bounds(stale_scale, 1.25, 1_200, 750),
            Err(ViewportBoundsError::StaleScaleFactor)
        );
        assert_eq!(
            serde_json::to_value(viewport_error(ViewportBoundsError::StaleScaleFactor))
                .expect("serialize stale-scale error")["code"],
            "output.viewport_scale_stale"
        );
    }

    #[test]
    fn fractional_dpi_fullscreen_edge_noise_is_clamped_to_client() {
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
        let request = validate_viewport_bounds(fullscreen, 1.5, 2_560, 1_440)
            .expect("fractional fullscreen bounds");
        let bounds = request.bounds().expect("visible fullscreen bounds");
        assert_eq!((bounds.width(), bounds.height()), (2_560, 1_440));
    }

    #[test]
    fn a_new_epoch_hides_with_a_monotonic_host_revision() {
        let store = EmbeddedViewportStore::new();
        let (first, first_hidden) = store.begin_session().expect("first session");
        let first_visible = store
            .apply(validated(first.epoch, 1, 1.0))
            .expect("visible");
        let (reloaded, reloaded_hidden) = store.begin_session().expect("reload session");

        assert!(reloaded.epoch > first.epoch);
        assert!(first_visible.revision() > first_hidden.revision());
        assert!(reloaded_hidden.revision() > first_visible.revision());
        assert_eq!(store.current().expect("current"), reloaded_hidden);
        assert_eq!(reloaded_hidden.bounds(), None);
    }

    #[test]
    fn a_stale_epoch_and_stale_client_revision_are_rejected() {
        let store = EmbeddedViewportStore::new();
        let (first, _) = store.begin_session().expect("first session");
        let (second, _) = store.begin_session().expect("second session");
        let stale_epoch = store
            .apply(validated(first.epoch, 1, 1.0))
            .expect_err("old session must fail");
        assert_eq!(
            serde_json::to_value(stale_epoch).expect("serialize")["code"],
            "output.viewport_session_stale"
        );

        store
            .apply(validated(second.epoch, 2, 1.0))
            .expect("newer revision");
        let stale_revision = store
            .apply(validated(second.epoch, 1, 1.0))
            .expect_err("stale revision must fail");
        assert_eq!(
            serde_json::to_value(stale_revision).expect("serialize")["code"],
            "output.viewport_revision_stale"
        );
    }

    #[test]
    fn same_revision_is_only_an_exact_idempotent_retry() {
        let store = EmbeddedViewportStore::new();
        let (session, _) = store.begin_session().expect("session");
        let validated_request = validated(session.epoch, 1, 1.0);
        let first = store.apply(validated_request).expect("first placement");
        assert_eq!(store.apply(validated_request).expect("exact retry"), first);

        let mut conflicting = request(session.epoch, 1, 1.0);
        conflicting.width_css += 0.25;
        let conflicting = validate_viewport_bounds(conflicting, 1.0, 2_000, 2_000)
            .expect("valid conflicting geometry");
        let error = store
            .apply(conflicting)
            .expect_err("same revision conflict");
        assert_eq!(
            serde_json::to_value(error).expect("serialize")["code"],
            "output.viewport_revision_conflict"
        );
    }

    #[test]
    fn placement_confirmation_rejects_a_superseding_epoch() {
        let store = EmbeddedViewportStore::new();
        let (first, _) = store.begin_session().expect("first session");
        let request = validated(first.epoch, 1, 1.0);
        let viewport = store.apply(request).expect("first placement");
        let _ = store.begin_session().expect("superseding session");

        let error = store
            .confirm_applied(request, viewport)
            .expect_err("superseded placement must not be acknowledged");
        assert_eq!(
            serde_json::to_value(error).expect("serialize")["code"],
            "output.viewport_session_stale"
        );
    }

    #[test]
    fn hidden_zero_extent_is_not_load_ready() {
        let store = EmbeddedViewportStore::new();
        let (session, _) = store.begin_session().expect("session");
        let hidden = ViewportBoundsRequest {
            epoch: session.epoch,
            revision: 1,
            x_css: 0.0,
            y_css: 0.0,
            width_css: 0.0,
            height_css: 0.0,
            scale_factor: 1.0,
            visible: false,
        };
        let hidden = validate_viewport_bounds(hidden, 1.0, 960, 600).expect("hidden");
        let hidden = store.apply(hidden).expect("store hidden");
        assert_eq!(hidden.bounds(), None);
        let error = store
            .current_visible()
            .expect_err("hidden is not load-ready");
        assert_eq!(
            serde_json::to_value(error).expect("serialize command error")["code"],
            "output.viewport_not_ready"
        );
    }
}
