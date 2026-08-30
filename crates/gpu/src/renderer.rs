//! DX12-only wgpu context and validated RGBA8 presentation primitives.

use std::borrow::Cow;

use thiserror::Error;

use crate::ring::{RgbaFrame, RingError, RingLayout};

/// Texture format used between the decoded-frame upload and presentation pass.
pub const FRAME_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Integer-pixel aspect-fit rectangle inside a non-zero presentation target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AspectFitViewport {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl AspectFitViewport {
    /// Left edge in physical target pixels.
    #[must_use]
    pub const fn x(self) -> u32 {
        self.x
    }

    /// Top edge in physical target pixels.
    #[must_use]
    pub const fn y(self) -> u32 {
        self.y
    }

    /// Fitted width in physical target pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Fitted height in physical target pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }
}

/// Calculate a centered integer viewport without cropping or changing aspect.
///
/// The unused target area is intentionally left for a black clear, yielding
/// letterboxing or pillarboxing while the intrinsic frame texture stays exact.
///
/// # Errors
///
/// Returns an error for zero source/target dimensions or an impossible integer
/// conversion.
pub fn aspect_fit_viewport(
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> Result<AspectFitViewport, RendererError> {
    if source_width == 0 || source_height == 0 || target_width == 0 || target_height == 0 {
        return Err(RendererError::ZeroSurfaceSize);
    }
    let source_across_target = u64::from(source_width) * u64::from(target_height);
    let target_across_source = u64::from(target_width) * u64::from(source_height);
    let (width, height) = if source_across_target > target_across_source {
        let height =
            (u64::from(target_width) * u64::from(source_height) / u64::from(source_width)).max(1);
        (
            target_width,
            u32::try_from(height).map_err(|_| RendererError::PresentationViewportOverflow)?,
        )
    } else {
        let width =
            (u64::from(target_height) * u64::from(source_width) / u64::from(source_height)).max(1);
        (
            u32::try_from(width).map_err(|_| RendererError::PresentationViewportOverflow)?,
            target_height,
        )
    };
    Ok(AspectFitViewport {
        x: (target_width - width) / 2,
        y: (target_height - height) / 2,
        width,
        height,
    })
}

/// Returns the exact native instance policy used by `LatentDeck` presentation.
///
/// The caller may safely create a `wgpu::Surface` from any owned raw-window-
/// handle provider (including a plain Tauri `Window`) without this crate
/// depending on the window toolkit.
#[must_use]
pub fn dx12_instance_descriptor() -> wgpu::InstanceDescriptor {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::DX12;
    descriptor
}

/// Creates a DX12-only wgpu instance without panicking when that backend was
/// not compiled for the current target.
///
/// # Errors
///
/// Returns [`RendererError::Dx12BackendNotCompiled`] outside a compatible
/// native build.
pub fn create_dx12_instance() -> Result<wgpu::Instance, RendererError> {
    if !wgpu::Instance::enabled_backend_features().contains(wgpu::Backends::DX12) {
        return Err(RendererError::Dx12BackendNotCompiled);
    }
    Ok(wgpu::Instance::new(dx12_instance_descriptor()))
}

/// Borrowed, fully checked RGBA8 upload data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RgbaUpload<'a> {
    layout: RingLayout,
    bytes: &'a [u8],
}

impl<'a> RgbaUpload<'a> {
    /// Validates a padded RGBA8 frame before any GPU API call.
    ///
    /// # Errors
    ///
    /// Returns an error unless dimensions fit RGB Ring ABI 1, `row_stride` is
    /// the exact 256-byte-aligned stride, and `bytes` covers every padded row.
    pub fn new(
        width: u32,
        height: u32,
        row_stride: u32,
        bytes: &'a [u8],
    ) -> Result<Self, UploadError> {
        let layout = RingLayout::new(width, height).map_err(UploadError::InvalidLayout)?;
        if row_stride != layout.row_stride() {
            return Err(UploadError::InvalidRowStride {
                expected: layout.row_stride(),
                actual: row_stride,
            });
        }
        let expected = usize::try_from(layout.payload_bytes())
            .map_err(|_| UploadError::InvalidLayout(RingError::LayoutOverflow))?;
        if bytes.len() != expected {
            return Err(UploadError::InvalidDataLength {
                expected,
                actual: bytes.len(),
            });
        }
        Ok(Self { layout, bytes })
    }

    /// Frame width in pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.layout.width()
    }

    /// Frame height in pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.layout.height()
    }

    /// Padded bytes per row.
    #[must_use]
    pub const fn row_stride(self) -> u32 {
        self.layout.row_stride()
    }

    /// RGBA8 bytes including row padding.
    #[must_use]
    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    /// Validates and borrows a frame copied from RGB Ring ABI 1.
    ///
    /// # Errors
    ///
    /// Returns an error if a future or corrupted frame violates the current
    /// upload contract.
    pub fn from_ring_frame(frame: &'a RgbaFrame) -> Result<Self, UploadError> {
        Self::new(
            frame.width(),
            frame.height(),
            frame.row_stride(),
            frame.padded_rgba(),
        )
    }
}

/// Pre-GPU frame validation failure.
#[derive(Debug, Error)]
pub enum UploadError {
    /// The dimensions cannot be represented by RGB Ring ABI 1.
    #[error("invalid RGBA upload layout: {0}")]
    InvalidLayout(#[source] RingError),
    /// The row stride is not the unique ABI-aligned stride.
    #[error("RGBA upload row stride is {actual}, expected {expected}")]
    InvalidRowStride { expected: u32, actual: u32 },
    /// The mapped payload is truncated or has trailing bytes.
    #[error("RGBA upload has {actual} bytes, expected {expected}")]
    InvalidDataLength { expected: usize, actual: usize },
    /// The upload dimensions do not match the allocated frame texture.
    #[error(
        "RGBA upload is {actual_width}x{actual_height}, expected {expected_width}x{expected_height}"
    )]
    FrameSizeMismatch {
        expected_width: u32,
        expected_height: u32,
        actual_width: u32,
        actual_height: u32,
    },
}

/// DX12 adapter, device, and direct queue chosen against an optional surface.
pub struct Dx12Device {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl Dx12Device {
    /// Requests a high-performance DX12 adapter compatible with `surface`.
    /// Passing a surface created from an owned Tauri raw `Window` requires no
    /// Tauri type at this boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when no compatible adapter/device exists or a backend
    /// other than DX12 is unexpectedly selected.
    pub async fn request(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self, RendererError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: surface,
                apply_limit_buckets: false,
            })
            .await?;
        let info = adapter.get_info();
        if info.backend != wgpu::Backend::Dx12 {
            return Err(RendererError::UnexpectedBackend(info.backend));
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("LatentDeck DX12 presentation device"),
                ..Default::default()
            })
            .await?;
        Ok(Self {
            adapter,
            device,
            queue,
        })
    }

    /// Selected adapter.
    #[must_use]
    pub const fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    /// Logical device used by presentation resources.
    #[must_use]
    pub const fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Direct presentation queue.
    #[must_use]
    pub const fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Produces the default FIFO surface configuration for a non-zero physical
    /// window size. The caller owns `Surface::configure` and resize timing.
    ///
    /// # Errors
    ///
    /// Returns an error for zero dimensions or an incompatible surface.
    pub fn surface_configuration(
        &self,
        surface: &wgpu::Surface<'_>,
        width: u32,
        height: u32,
    ) -> Result<wgpu::SurfaceConfiguration, RendererError> {
        if width == 0 || height == 0 {
            return Err(RendererError::ZeroSurfaceSize);
        }
        let mut configuration = surface
            .get_default_config(&self.adapter, width, height)
            .ok_or(RendererError::SurfaceUnsupported)?;
        configuration.present_mode = wgpu::PresentMode::Fifo;
        configuration.desired_maximum_frame_latency = 2;
        Ok(configuration)
    }
}

/// Uploads ABI-padded RGBA8 and draws it with one fullscreen triangle.
pub struct RgbaFrameRenderer {
    frame_layout: RingLayout,
    frame_texture: wgpu::Texture,
    frame_bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
}

impl RgbaFrameRenderer {
    /// Allocates a fixed-size RGBA8 frame texture and presentation pipeline for
    /// `target_format`.
    ///
    /// # Errors
    ///
    /// Returns an error when frame dimensions violate RGB Ring ABI 1 limits.
    pub fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        frame_width: u32,
        frame_height: u32,
    ) -> Result<Self, RendererError> {
        let frame_layout = RingLayout::new(frame_width, frame_height)
            .map_err(RendererError::InvalidFrameLayout)?;
        let (frame_texture, frame_bind_group, bind_group_layout) =
            create_frame_resources(device, frame_width, frame_height);
        let pipeline = create_presentation_pipeline(device, target_format, &bind_group_layout);

        Ok(Self {
            frame_layout,
            frame_texture,
            frame_bind_group,
            pipeline,
        })
    }

    /// Uploads one validated padded frame into the fixed texture.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame dimensions differ from the renderer's
    /// allocated program texture.
    pub fn upload(&self, queue: &wgpu::Queue, upload: RgbaUpload<'_>) -> Result<(), UploadError> {
        if upload.width() != self.frame_layout.width()
            || upload.height() != self.frame_layout.height()
        {
            return Err(UploadError::FrameSizeMismatch {
                expected_width: self.frame_layout.width(),
                expected_height: self.frame_layout.height(),
                actual_width: upload.width(),
                actual_height: upload.height(),
            });
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.frame_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            upload.bytes(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(upload.row_stride()),
                rows_per_image: Some(upload.height()),
            },
            wgpu::Extent3d {
                width: upload.width(),
                height: upload.height(),
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    /// Records a black clear and one fullscreen triangle into `target`.
    ///
    /// This exact-target primitive remains useful for same-size/offscreen
    /// targets. Resizable windows should call [`Self::encode_aspect_fit`].
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        self.encode_with_viewport(encoder, target, None);
    }

    /// Records a black clear, then centers the intrinsic frame without crop or
    /// aspect distortion inside the current non-zero presentation extent.
    ///
    /// # Errors
    ///
    /// Returns an error when the presentation extent is zero or cannot be
    /// represented safely.
    pub fn encode_aspect_fit(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        target_width: u32,
        target_height: u32,
    ) -> Result<(), RendererError> {
        let viewport = aspect_fit_viewport(
            self.frame_layout.width(),
            self.frame_layout.height(),
            target_width,
            target_height,
        )?;
        self.encode_with_viewport(encoder, target, Some(viewport));
        Ok(())
    }

    #[allow(clippy::cast_precision_loss)] // wgpu viewport coordinates are f32.
    fn encode_with_viewport(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: Option<AspectFitViewport>,
    ) {
        let color_attachment = Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        });
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("LatentDeck frame presentation"),
            color_attachments: &[color_attachment],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.frame_bind_group, &[]);
        if let Some(viewport) = viewport {
            render_pass.set_viewport(
                viewport.x as f32,
                viewport.y as f32,
                viewport.width as f32,
                viewport.height as f32,
                0.0,
                1.0,
            );
        }
        render_pass.draw(0..3, 0..1);
    }

    /// Program texture kept independent from any resizable swapchain image.
    #[must_use]
    pub const fn frame_texture(&self) -> &wgpu::Texture {
        &self.frame_texture
    }

    /// Fixed decoded-program layout.
    #[must_use]
    pub const fn frame_layout(&self) -> RingLayout {
        self.frame_layout
    }
}

fn create_frame_resources(
    device: &wgpu::Device,
    frame_width: u32,
    frame_height: u32,
) -> (wgpu::Texture, wgpu::BindGroup, wgpu::BindGroupLayout) {
    let frame_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("LatentDeck RGBA frame"),
        size: wgpu::Extent3d {
            width: frame_width,
            height: frame_height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FRAME_TEXTURE_FORMAT,
        usage: wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let frame_view = frame_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("LatentDeck frame sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("LatentDeck frame bindings"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("LatentDeck frame bind group"),
        layout: &bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&frame_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    (frame_texture, frame_bind_group, bind_group_layout)
}

fn create_presentation_pipeline(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("LatentDeck presentation pipeline layout"),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("LatentDeck fullscreen frame shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("fullscreen.wgsl"))),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("LatentDeck fullscreen frame pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fragment_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Native context or pipeline construction failure.
#[derive(Debug, Error)]
pub enum RendererError {
    /// The current target was built without wgpu's DX12 backend.
    #[error("wgpu DX12 backend is not compiled for this target")]
    Dx12BackendNotCompiled,
    /// No adapter satisfies the surface/backend request.
    #[error("failed to request DX12 adapter: {0}")]
    RequestAdapter(#[from] wgpu::RequestAdapterError),
    /// The selected adapter could not create a logical device.
    #[error("failed to request DX12 device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    /// A non-DX12 adapter escaped the fixed instance policy.
    #[error("unexpected wgpu backend {0:?}; DX12 is required")]
    UnexpectedBackend(wgpu::Backend),
    /// Surface configuration cannot use zero physical dimensions.
    #[error("surface dimensions must be non-zero")]
    ZeroSurfaceSize,
    /// Aspect-fit presentation geometry exceeded its integer representation.
    #[error("presentation viewport dimensions overflow")]
    PresentationViewportOverflow,
    /// The chosen surface and adapter expose no compatible configuration.
    #[error("surface is not compatible with the selected DX12 adapter")]
    SurfaceUnsupported,
    /// Program texture dimensions violate the bounded ring layout.
    #[error("invalid renderer frame layout: {0}")]
    InvalidFrameLayout(#[source] RingError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portrait_frame_is_pillarboxed_inside_a_wide_surface() {
        assert_eq!(
            aspect_fit_viewport(448, 800, 1_600, 900).expect("viewport"),
            AspectFitViewport {
                x: 548,
                y: 0,
                width: 504,
                height: 900,
            }
        );
    }

    #[test]
    fn landscape_frame_is_letterboxed_inside_a_tall_surface() {
        assert_eq!(
            aspect_fit_viewport(800, 448, 900, 1_600).expect("viewport"),
            AspectFitViewport {
                x: 0,
                y: 548,
                width: 900,
                height: 504,
            }
        );
    }

    #[test]
    fn equal_aspect_fills_the_entire_surface() {
        assert_eq!(
            aspect_fit_viewport(800, 448, 1_600, 896).expect("viewport"),
            AspectFitViewport {
                x: 0,
                y: 0,
                width: 1_600,
                height: 896,
            }
        );
    }
}
