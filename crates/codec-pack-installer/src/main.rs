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
    /// Legacy Setup argument retained for command-line compatibility. Protocol
    /// 2 packages install only below the current-user Local `AppData` root.
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
    },
    /// Explicitly repair one exact H3 Codec Pack from a build-authorized archive.
    Repair {
        #[arg(long)]
        archive: PathBuf,
    },
    /// Verify one exact installed H3 Codec Pack version.
    Verify {
        #[arg(long)]
        version: String,
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
        Command::Install { archive } => {
            let receipt = latentdeck_codec_pack_installer::install(
                &roots,
                &InstallRequest {
                    archive_path: archive,
                },
            )?;
            Ok(format!(
                "installed org.latentdeck.h3 {} at {}",
                receipt.pack_version,
                receipt.destination.display()
            ))
        }
        Command::Repair { archive } => {
            let receipt = latentdeck_codec_pack_installer::repair(
                &roots,
                &InstallRequest {
                    archive_path: archive,
                },
            )?;
            Ok(format!(
                "repaired org.latentdeck.h3 {} at {}",
                receipt.pack_version,
                receipt.destination.display()
            ))
        }
        Command::Verify { version } => {
            let receipt = latentdeck_codec_pack_installer::verify(&roots, &version)?;
            Ok(format!(
                "verified org.latentdeck.h3 {} at {}",
                receipt.pack_version,
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
            Cli::try_parse_from(["codec-pack-installer", "uninstall", "--version", "0.2.0"])
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
            "0.2.0",
        ])
        .expect("explicit roots parse");
        assert_eq!(parsed.local_app_data, PathBuf::from(r"C:\ExplicitLocal"));
        assert_eq!(
            parsed.program_data,
            PathBuf::from(r"C:\ExplicitProgramData")
        );
    }

    #[test]
    fn install_cli_cannot_supply_reserved_namespace_authorization() {
        assert!(
            Cli::try_parse_from([
                "codec-pack-installer",
                "--local-app-data",
                r"C:\ExplicitLocal",
                "--program-data",
                r"C:\ExplicitProgramData",
                "install",
                "--archive",
                r"C:\payload.ldcodec",
                "--expected-sha256",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "--expected-length",
                "1234",
                "--expected-version",
                "0.2.0",
            ])
            .is_err()
        );
    }

    #[test]
    fn repair_and_verify_are_explicit_subcommands() {
        let repair = Cli::try_parse_from([
            "codec-pack-installer",
            "--local-app-data",
            r"C:\ExplicitLocal",
            "--program-data",
            r"C:\ExplicitProgramData",
            "repair",
            "--archive",
            r"C:\payload.ldcodec",
        ])
        .expect("explicit repair parses");
        assert!(matches!(repair.command, super::Command::Repair { .. }));

        let verify = Cli::try_parse_from([
            "codec-pack-installer",
            "--local-app-data",
            r"C:\ExplicitLocal",
            "--program-data",
            r"C:\ExplicitProgramData",
            "verify",
            "--version",
            "0.2.0",
        ])
        .expect("explicit verify parses");
        assert!(matches!(verify.command, super::Command::Verify { .. }));
    }
}
