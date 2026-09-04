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

/// Small supported facade for ordinary LC inspection, validation, authoring,
/// hashing, and genealogy-aware resampling. Low-level modules remain public
/// for specialized implementations, but new integrations should start here.
pub mod sdk {
    pub use crate::LC_SPEC_VERSION;
    pub use crate::authoring::{
        RawH3AuthoringOptions, RawH3Inspection, inspect_raw_h3, pack_raw_h3_atomic,
    };
    pub use crate::error::{CartridgeError, ErrorCode, ErrorLocation, Result};
    pub use crate::hash::{MeasuredHash, Sha256Hash, hash_path};
    pub use crate::limits::ValidationLimits;
    pub use crate::manifest::{
        AudioDisposition, AudioOmissionReason, CartridgeId, CodecDescriptor, DType, Identifier,
        ManifestV0_1, OperationRecord, ParentCartridge, PayloadDescriptor, ProducerDescriptor,
        Provenance, ProvenanceSource, Rational, Sha256Digest, SourceCartridgeRef, SpecVersion,
        TensorDescriptor, TensorStream, TimingDescriptor,
    };
    pub use crate::reader::{
        CartridgeInspection, InspectOptions, IntegrityValidatedCartridge,
        IntegrityValidationReceipt, ValidatedCartridge, ValidationLevel, ValidationOptions,
        ValidationReceipt, inspect_path, open_integrity_validated, open_validated,
    };
    pub use crate::resample::{
        CaptureMode, PayloadExpectation, ProfileResampleRequest, ProfileResampleWriteReceipt,
        ResampleManifestRequest, ResampleWriteReceipt, build_resample_manifest,
        pack_profile_resample_atomic, pack_resample_atomic,
    };
    pub use crate::writer::{
        OverwritePolicy, PackRequest, WriteOptions, WriteReceipt, pack_atomic,
    };
}

/// The implemented `Latent Cartridge` specification version.
pub const LC_SPEC_VERSION: &str = "0.1.0";
