use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use latentdeck_extension_manager::{
    ErrorCode, ExtensionError, ExtensionRoots, InstallRequest, PackRequest, PackageKind,
    PackageReference, RemoveOptions, compatibility_matrix, disable, enable, inspect, install, list,
    pack, remove, repair, verify,
};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(name = "latentdeck-extension-manager", version)]
struct Cli {
    /// Explicit LOCALAPPDATA known-folder path. Lifecycle commands fall back to
    /// the process LOCALAPPDATA value when this option is absent.
    #[arg(long, global = true)]
    local_app_data: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Inspect {
        #[arg(long)]
        archive: PathBuf,
        #[arg(long)]
        expected_sha256: Option<String>,
    },
    Pack {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Install {
        #[arg(long)]
        archive: PathBuf,
        #[arg(long)]
        expected_sha256: String,
    },
    Verify(PackageSelector),
    Enable(PackageSelector),
    Disable(PackageSelector),
    Remove {
        #[command(flatten)]
        package: PackageSelector,
        #[arg(long)]
        allow_corrupt: bool,
    },
    Repair {
        #[arg(long)]
        archive: PathBuf,
        #[arg(long)]
        expected_sha256: String,
    },
    List,
    Matrix,
}

#[derive(Debug, Clone, Args)]
struct PackageSelector {
    #[arg(long)]
    kind: KindArgument,
    #[arg(long)]
    id: String,
    #[arg(long)]
    version: String,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum KindArgument {
    Deck,
    Codec,
}

impl From<PackageSelector> for PackageReference {
    fn from(value: PackageSelector) -> Self {
        Self {
            kind: match value.kind {
                KindArgument::Deck => PackageKind::DeckPack,
                KindArgument::Codec => PackageKind::CodecPack,
            },
            package_id: value.id,
            package_version: value.version,
        }
    }
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return;
        }
        Err(error) => {
            eprintln!("latentdeck-extension-manager: invalid arguments");
            let _ = error;
            std::process::exit(10);
        }
    };
    match run(cli) {
        Ok(json) => println!("{json}"),
        Err(error) => {
            eprintln!(
                "latentdeck-extension-manager: {}: {}",
                error.code().as_str(),
                error.detail()
            );
            std::process::exit(i32::from(error.exit_code()));
        }
    }
}

fn run(cli: Cli) -> Result<String, ExtensionError> {
    match cli.command {
        Command::Inspect {
            archive,
            expected_sha256,
        } => encode(&inspect(archive, expected_sha256.as_deref())?),
        Command::Pack { source, output } => encode(&pack(&PackRequest {
            source_directory: source,
            output_path: output,
        })?),
        command => {
            let roots = roots(cli.local_app_data)?;
            match command {
                Command::Install {
                    archive,
                    expected_sha256,
                } => encode(&install(
                    &roots,
                    &InstallRequest {
                        archive_path: archive,
                        expected_sha256,
                    },
                )?),
                Command::Verify(package) => encode(&verify(&roots, &package.into())?),
                Command::Enable(package) => encode(&enable(&roots, &package.into())?),
                Command::Disable(package) => encode(&disable(&roots, &package.into())?),
                Command::Remove {
                    package,
                    allow_corrupt,
                } => encode(&remove(
                    &roots,
                    &package.into(),
                    RemoveOptions { allow_corrupt },
                )?),
                Command::Repair {
                    archive,
                    expected_sha256,
                } => encode(&repair(
                    &roots,
                    &InstallRequest {
                        archive_path: archive,
                        expected_sha256,
                    },
                )?),
                Command::List => encode(&list(&roots)?),
                Command::Matrix => encode(&compatibility_matrix(&roots)?),
                Command::Inspect { .. } | Command::Pack { .. } => unreachable!(),
            }
        }
    }
}

fn roots(explicit: Option<PathBuf>) -> Result<ExtensionRoots, ExtensionError> {
    let local_app_data = explicit
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .ok_or_else(|| {
            ExtensionError::new(
                ErrorCode::InvalidArguments,
                "lifecycle command requires --local-app-data or LOCALAPPDATA",
            )
        })?;
    Ok(ExtensionRoots::from_local_app_data(local_app_data))
}

fn encode<T: Serialize>(value: &T) -> Result<String, ExtensionError> {
    serde_json::to_string(value).map_err(|error| {
        ExtensionError::new(
            ErrorCode::LifecycleConflict,
            format!("serialize command result: {error}"),
        )
    })
}
