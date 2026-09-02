//! Native GPU presentation and bounded shared-frame transport.

pub mod renderer;
pub mod ring;
pub mod ring_v2;
#[cfg(target_os = "windows")]
pub mod windows_ring;
#[cfg(target_os = "windows")]
pub mod windows_ring_v2;

/// The pinned presentation backend API generation.
pub const WGPU_API_MAJOR: u32 = 30;
