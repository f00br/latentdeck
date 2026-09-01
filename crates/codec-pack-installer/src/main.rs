use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, error::ErrorKind};
use latentdeck_codec_pack_installer::{InstallRequest, LifecycleError, LifecycleRoots};

#[derive(Debug, Parser)]
#[command(name = "latentdeck-codec-pack-installer", version)]
#[command(about = "Native LatentDeck H3 Codec Pack lifecycle helper")]
struct Cli {
    /// Explicit current-user Local `AppData` known-folder path from the wrapper.
    #[arg(long, value_name = "PATH")]
    local_app_data: PathBuf,
    /// Explicit all-users `ProgramData` known-folder path from the wrapper.
    #[arg(long, value_name = "PATH")]
    program_data: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Install one exact immutable H3 Codec Pack archive.
    Install {
        #[arg(long)]
        archive: PathBuf,
        #[arg(long)]
        expected_sha256: String,
        #[arg(long)]
        expected_length: u64,
        #[arg(long)]
        expected_version: String,
    },
    /// Uninstall one exact H3 Codec Pack version.
    Uninstall {
        #[arg(long)]
        version: String,
        #[arg(long)]
        remove_corrupt: bool,
    },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return ExitCode::SUCCESS;
        }
        Err(_) => {
            eprintln!("codec-pack-installer: invalid arguments");
            return ExitCode::from(latentdeck_codec_pack_installer::EXIT_INVALID_ARGUMENTS);
        }
    };
    match run(cli) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            let message = error.to_string().replace(['\r', '\n'], " ");
            eprintln!("codec-pack-installer: {message}");
            ExitCode::from(error.exit_code())
        }
    }
}

fn run(cli: Cli) -> Result<String, LifecycleError> {
    let roots = LifecycleRoots::from_known_folders(cli.local_app_data, cli.program_data);
    match cli.command {
        Command::Install {
            archive,
            expected_sha256,
            expected_length,
            expected_version,
        } => {
            let receipt = latentdeck_codec_pack_installer::install(
                &roots,
                &InstallRequest {
                    archive_path: archive,
                    expected_sha256,
                    expected_length,
                    expected_version: expected_version.clone(),
                },
            )?;
            Ok(format!(
                "installed org.latentdeck.h3 {expected_version} at {}",
                receipt.destination.display()
            ))
        }
        Command::Uninstall {
            version,
            remove_corrupt,
        } => {
            let receipt =
                latentdeck_codec_pack_installer::uninstall(&roots, &version, remove_corrupt)?;
            if receipt.cleaned_quarantine {
                Ok(format!(
                    "cleaned quarantine for org.latentdeck.h3 {}",
                    receipt.removed_version
                ))
            } else {
                Ok(format!(
                    "removed org.latentdeck.h3 {}",
                    receipt.removed_version
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::Cli;
    use clap::{CommandFactory, Parser};

    #[test]
    fn clap_contract_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn explicit_known_folder_roots_are_required() {
        assert!(
            Cli::try_parse_from(["codec-pack-installer", "uninstall", "--version", "0.1.1"])
                .is_err()
        );
        let parsed = Cli::try_parse_from([
            "codec-pack-installer",
            "--local-app-data",
            r"C:\ExplicitLocal",
            "--program-data",
            r"C:\ExplicitProgramData",
            "uninstall",
            "--version",
            "0.1.1",
        ])
        .expect("explicit roots parse");
        assert_eq!(parsed.local_app_data, PathBuf::from(r"C:\ExplicitLocal"));
        assert_eq!(
            parsed.program_data,
            PathBuf::from(r"C:\ExplicitProgramData")
        );
    }
}
