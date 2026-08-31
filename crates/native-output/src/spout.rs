use latentdeck_output_spout::{SenderConfig, SpoutFormat};

#[cfg(all(windows, feature = "spout-sdk"))]
use latentdeck_output_spout::{SenderStatus, SpoutError, SpoutSender};

#[cfg(any(test, all(windows, feature = "spout-sdk")))]
use latentdeck_output_spout::BackendFault;

use crate::{NativeOutputError, NativeSpoutStatus};

const FORMAT_TOKEN: &str = "rgba8_unorm";

pub(crate) struct SpoutSurface {
    status: NativeSpoutStatus,
    #[cfg(all(windows, feature = "spout-sdk"))]
    sender: Option<SpoutSender>,
}

impl SpoutSurface {
    pub(crate) fn open(name: &str, width: u32, height: u32, device: &wgpu::Device) -> Self {
        let config = SenderConfig::new(name, width, height, SpoutFormat::Rgba8Unorm);
        let mut surface = Self {
            status: initial_status(name, width, height),
            #[cfg(all(windows, feature = "spout-sdk"))]
            sender: None,
        };
        let Ok(config) = config else {
            surface.status.last_error_code = Some("output.spout_name_invalid");
            return surface;
        };

        #[cfg(all(windows, feature = "spout-sdk"))]
        match raw_wgpu::open_sender(config, device) {
            Ok(sender) => {
                surface.status.ready = true;
                surface.update(sender.status());
                surface.sender = Some(sender);
            }
            Err(error) => surface.record_error(&error),
        }

        #[cfg(not(all(windows, feature = "spout-sdk")))]
        let _ = (config, device);

        surface
    }

    pub(crate) fn status(&self) -> NativeSpoutStatus {
        self.status.clone()
    }

    pub(crate) fn set_enabled(
        &mut self,
        enabled: bool,
    ) -> Result<NativeSpoutStatus, NativeOutputError> {
        #[cfg(all(windows, feature = "spout-sdk"))]
        {
            let Some(sender) = self.sender.as_mut() else {
                self.status.last_error_code = Some("output.spout_unavailable");
                return Err(NativeOutputError::SpoutUnavailable);
            };
            match sender.set_enabled(enabled) {
                Ok(status) => {
                    self.update(status);
                    Ok(self.status())
                }
                Err(error) => {
                    self.record_error(&error);
                    Err(NativeOutputError::SpoutControl)
                }
            }
        }
        #[cfg(not(all(windows, feature = "spout-sdk")))]
        {
            if !enabled {
                self.status.enabled = false;
                return Ok(self.status());
            }
            self.status.last_error_code = Some("output.spout_sdk_not_built");
            Err(NativeOutputError::SpoutUnavailable)
        }
    }

    pub(crate) fn set_name(&mut self, name: &str) -> Result<NativeSpoutStatus, NativeOutputError> {
        if SenderConfig::new(
            name,
            self.status.width,
            self.status.height,
            SpoutFormat::Rgba8Unorm,
        )
        .is_err()
        {
            self.status.last_error_code = Some("output.spout_name_invalid");
            return Err(NativeOutputError::SpoutControl);
        }

        #[cfg(all(windows, feature = "spout-sdk"))]
        {
            let Some(sender) = self.sender.as_mut() else {
                self.status.last_error_code = Some("output.spout_unavailable");
                return Err(NativeOutputError::SpoutUnavailable);
            };
            match sender.set_name(name) {
                Ok(status) => {
                    self.update(status);
                    Ok(self.status())
                }
                Err(error) => {
                    self.record_error(&error);
                    Err(NativeOutputError::SpoutControl)
                }
            }
        }
        #[cfg(not(all(windows, feature = "spout-sdk")))]
        {
            let _ = name;
            self.status.last_error_code = Some("output.spout_sdk_not_built");
            Err(NativeOutputError::SpoutUnavailable)
        }
    }

    pub(crate) fn submit(&mut self, texture: &wgpu::Texture) {
        #[cfg(all(windows, feature = "spout-sdk"))]
        {
            let Some(sender) = self.sender.as_mut() else {
                return;
            };
            if !sender.status().enabled {
                return;
            }
            let Some(sequence) = sender.status().last_sequence.unwrap_or(0).checked_add(1) else {
                self.status.last_error_code = Some("output.spout_sequence_exhausted");
                return;
            };
            match raw_wgpu::send_texture(sender, sequence, texture) {
                Ok(status) => self.update(status),
                Err(error) => self.record_error(&error),
            }
        }
        #[cfg(not(all(windows, feature = "spout-sdk")))]
        let _ = (self, texture);
    }

    #[cfg(all(windows, feature = "spout-sdk"))]
    fn update(&mut self, status: SenderStatus) {
        self.status.enabled = status.enabled;
        self.status.published = status.published;
        self.status.requested_name = status.requested_name;
        self.status.active_name = status.active_name;
        self.status.submitted_frames = status.submitted_frames;
        self.status.last_sequence = status.last_sequence;
        self.status.spout_frame = status.spout_frame;
        self.status.last_error_code = status.last_fault.map(backend_fault_code);
    }

    #[cfg(all(windows, feature = "spout-sdk"))]
    fn record_error(&mut self, error: &SpoutError) {
        self.status.last_error_code = Some(error_code(error));
    }
}

fn initial_status(name: &str, width: u32, height: u32) -> NativeSpoutStatus {
    NativeSpoutStatus {
        sdk_built: cfg!(all(windows, feature = "spout-sdk")),
        ready: false,
        enabled: false,
        published: false,
        requested_name: name.to_owned(),
        active_name: name.to_owned(),
        width,
        height,
        format: FORMAT_TOKEN,
        submitted_frames: 0,
        last_sequence: None,
        spout_frame: None,
        last_error_code: (!cfg!(all(windows, feature = "spout-sdk")))
            .then_some("output.spout_sdk_not_built"),
    }
}

#[cfg(any(test, all(windows, feature = "spout-sdk")))]
fn error_code(error: &latentdeck_output_spout::SpoutError) -> &'static str {
    use latentdeck_output_spout::SpoutError;
    match error {
        SpoutError::InvalidName => "output.spout_name_invalid",
        SpoutError::InvalidDimensions { .. } => "output.spout_dimensions_invalid",
        SpoutError::SdkNotBuilt => "output.spout_sdk_not_built",
        SpoutError::Disabled => "output.spout_disabled",
        SpoutError::Stopped => "output.spout_stopped",
        SpoutError::NonMonotonicSequence { .. } => "output.spout_sequence_invalid",
        SpoutError::Backend(fault) => backend_fault_code(*fault),
    }
}

#[cfg(any(test, all(windows, feature = "spout-sdk")))]
const fn backend_fault_code(fault: BackendFault) -> &'static str {
    match fault {
        BackendFault::IncompatibleDx12 => "output.spout_dx12_incompatible",
        BackendFault::OpenFailed => "output.spout_open_failed",
        BackendFault::ResourceMismatch => "output.spout_resource_mismatch",
        BackendFault::ResourceLimit => "output.spout_resource_limit",
        BackendFault::WrapFailed => "output.spout_wrap_failed",
        BackendFault::SendFailed => "output.spout_send_failed",
        BackendFault::StatusMismatch => "output.spout_status_invalid",
        BackendFault::Internal => "output.spout_internal",
    }
}

#[cfg(all(windows, feature = "spout-sdk"))]
mod raw_wgpu {
    #![allow(unsafe_code)]

    use latentdeck_output_spout::{
        Dx12ResourceState, SenderConfig, SenderStatus, SpoutError, SpoutSender,
    };

    pub(super) fn open_sender(
        config: SenderConfig,
        device: &wgpu::Device,
    ) -> Result<SpoutSender, SpoutError> {
        // SAFETY: the guard is held while cloning the COM interfaces. The
        // Spout bridge takes its own references and never destroys wgpu's
        // device or queue.
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }.ok_or(SpoutError::Backend(
            latentdeck_output_spout::BackendFault::IncompatibleDx12,
        ))?;
        let raw_device = hal.raw_device().clone();
        let raw_queue = hal.raw_queue().clone();
        SpoutSender::open_dx12(config, &raw_device, &raw_queue)
    }

    pub(super) fn send_texture(
        sender: &mut SpoutSender,
        sequence: u64,
        texture: &wgpu::Texture,
    ) -> Result<SenderStatus, SpoutError> {
        // SAFETY: the wgpu texture guard remains live through the synchronous
        // bridge call. The D3D11On12 wrapper acquires and releases the resource
        // with matching combined pixel/non-pixel shader-resource states and
        // takes its own bounded reference for future frames.
        let hal = unsafe { texture.as_hal::<wgpu::hal::api::Dx12>() }.ok_or(
            SpoutError::Backend(latentdeck_output_spout::BackendFault::IncompatibleDx12),
        )?;
        let resource = unsafe { hal.raw_resource() };
        sender.send_frame(sequence, resource, Dx12ResourceState::ShaderResource)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use latentdeck_output_spout::SpoutError;

    #[test]
    fn default_status_is_explicit_and_path_free() {
        let status = initial_status("LatentDeck", 800, 448);
        assert_eq!(status.requested_name, "LatentDeck");
        assert_eq!(status.width, 800);
        assert_eq!(status.height, 448);
        assert_eq!(status.format, "rgba8_unorm");
        assert!(!status.enabled);
        assert!(!status.published);
        if !cfg!(all(windows, feature = "spout-sdk")) {
            assert!(!status.sdk_built);
            assert_eq!(status.last_error_code, Some("output.spout_sdk_not_built"));
        }
    }

    #[test]
    fn every_spout_error_maps_to_a_stable_code() {
        assert_eq!(
            error_code(&SpoutError::InvalidName),
            "output.spout_name_invalid"
        );
        assert_eq!(
            error_code(&SpoutError::Backend(BackendFault::SendFailed)),
            "output.spout_send_failed"
        );
    }
}
