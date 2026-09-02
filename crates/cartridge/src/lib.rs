//! Stable, codec-neutral `Latent Cartridge` contracts.

pub mod access;
pub mod archive;
pub mod authoring;
pub mod error;
pub mod hash;
pub mod limits;
pub mod manifest;
pub mod preview;
pub mod profile;
pub mod reader;
pub mod resample;
pub mod safetensor;
pub mod signal;
pub mod writer;

/// The implemented `Latent Cartridge` specification version.
pub const LC_SPEC_VERSION: &str = "0.1.0";
