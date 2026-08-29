//! Native GPU presentation and bounded shared-frame transport.

pub mod renderer;
pub mod ring;
#[cfg(target_os = "windows")]
pub mod windows_ring;

/// The pinned presentation backend API generation.
pub const WGPU_API_MAJOR: u32 = 30;
