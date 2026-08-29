use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use latentdeck_cartridge::error::{CartridgeError, ErrorCode, Result};
use latentdeck_cartridge::hash::hash_path;
use latentdeck_cartridge::limits::ValidationLimits;
use latentdeck_cartridge::manifest::parse_manifest_json;
use latentdeck_cartridge::reader::{
    InspectOptions, ValidationOptions, inspect_path, open_validated,
};
use latentdeck_cartridge::safetensor::{
    H3SafetensorsPreflight, SafetensorDType, SafetensorTensorDescriptor,
};
use latentdeck_cartridge::writer::{OverwritePolicy, PackRequest, WriteOptions, pack_atomic};
use serde_json::{Value, json};

#[derive(Debug, Parser)]
#[command(
    name = "latentdeck-cartridge",
    version,
    about = "Pack, inspect, validate, and hash data-only Latent Cartridges"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Pack finalized manifest and payload inputs into one validated cartridge.
    Pack {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        payload: PathBuf,
        #[arg(long)]
        preview: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        overwrite: bool,
    },
    /// Inspect structure and metadata without granting tensor access.
    Inspect { path: PathBuf },
    /// Fully validate all bytes, hashes, and finite tensor values.
    Validate { path: PathBuf },
    /// Stream the complete-file SHA-256.
    Hash { path: PathBuf },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match execute(cli.command) {
        Ok(value) => {
            let mut output = std::io::stdout().lock();
            if serde_json::to_writer_pretty(&mut output, &value).is_err() {
                return ExitCode::from(6);
            }
            println!();
            ExitCode::SUCCESS
        }
        Err(error) => {
            let value = json!({
                "status": "error",
                "code": error.code(),
                "detail": error.detail,
                "location": error.location,
            });
            let mut output = std::io::stderr().lock();
            if serde_json::to_writer_pretty(&mut output, &value).is_ok() {
                eprintln!();
            }
            ExitCode::from(error.code.exit_status())
        }
    }
}

fn execute(command: Command) -> Result<Value> {
    match command {
        Command::Pack {
            manifest,
            payload,
            preview,
            output,
            overwrite,
        } => {
            let manifest = read_manifest(&manifest)?;
            let mut request = PackRequest::new(manifest, payload);
            if let Some(preview) = preview {
                request = request.with_preview(preview);
            }
            let options = WriteOptions {
                overwrite: if overwrite {
                    OverwritePolicy::Replace
                } else {
                    OverwritePolicy::Forbid
                },
            };
            let receipt = pack_atomic(&request, &output, &options)?;
            Ok(json!({
                "status": "ok",
                "command": "pack",
                "output": receipt.output_path,
                "validation": receipt.validation,
            }))
        }
        Command::Inspect { path } => {
            let inspection = inspect_path(&path, &InspectOptions::default())?;
            Ok(json!({
                "status": "ok",
                "command": "inspect",
                "validation_level": inspection.validation_level,
                "archive_bytes": inspection.archive_size,
                "manifest": inspection.manifest,
                "profile": {
                    "visual": {
                        "latent_slots": inspection.h3_profile.visual.latent_slots,
                        "latent_height": inspection.h3_profile.visual.latent_height,
                        "latent_width": inspection.h3_profile.visual.latent_width,
                        "decoded_frames": inspection.h3_profile.visual.decoded_frame_count,
                        "decoded_height": inspection.h3_profile.visual.decoded_height,
                        "decoded_width": inspection.h3_profile.visual.decoded_width,
                    },
                    "audio_latent_slots": inspection
                        .h3_profile
                        .audio
                        .as_ref()
                        .map(|audio| audio.latent_slots),
                },
                "safetensors": preflight_json(&inspection.safetensors),
            }))
        }
        Command::Validate { path } => {
            let validated = open_validated(&path, &ValidationOptions::default())?;
            Ok(json!({
                "status": "ok",
                "command": "validate",
                "cartridge_id": validated.manifest().cartridge_id.0,
                "validation": validated.receipt(),
            }))
        }
        Command::Hash { path } => {
            let measured = hash_path(path)?;
            Ok(json!({
                "status": "ok",
                "command": "hash",
                "byte_length": measured.byte_length,
                "sha256": measured.sha256,
            }))
        }
    }
}

fn read_manifest(path: &Path) -> Result<latentdeck_cartridge::manifest::ManifestV0_1> {
    let limits = ValidationLimits::default();
    let mut file = File::open(path).map_err(|error| {
        CartridgeError::new(ErrorCode::IoOpen, "cannot open manifest input").with_source(error)
    })?;
    let maximum = u64::try_from(limits.max_manifest_bytes()).map_err(|error| {
        CartridgeError::new(
            ErrorCode::ManifestTooLarge,
            "manifest limit does not fit u64",
        )
        .with_source(error)
    })?;
    let mut bounded = (&mut file).take(maximum.saturating_add(1));
    let mut bytes = Vec::new();
    bounded.read_to_end(&mut bytes).map_err(|error| {
        CartridgeError::new(ErrorCode::IoRead, "cannot read manifest input").with_source(error)
    })?;
    parse_manifest_json(&bytes, &limits)
}

fn preflight_json(preflight: &H3SafetensorsPreflight) -> Value {
    json!({
        "payload_bytes": preflight.payload_bytes,
        "header_bytes": preflight.header_bytes,
        "data_bytes": preflight.data_bytes,
        "video": tensor_json(&preflight.video),
        "audio": preflight.audio.as_ref().map(tensor_json),
    })
}

fn tensor_json(descriptor: &SafetensorTensorDescriptor) -> Value {
    json!({
        "name": descriptor.name,
        "dtype": match descriptor.dtype {
            SafetensorDType::F16 => "F16",
            SafetensorDType::F32 => "F32",
        },
        "shape": descriptor.shape,
        "data_offsets": descriptor.data_offsets,
        "byte_length": descriptor.byte_length,
    })
}
