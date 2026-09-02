use std::fs;

use latentdeck_extension_manager::{
    BundledPackageIndex, ErrorCode, ExtensionError, ExtensionRoots, InstallRequest,
    InstalledPackageSummary, PackageHealth, PackageKind, PackageReference,
    enable_if_only_installed_version, install_from_bundled_index, list, resolve_installed,
};

include!(concat!(
    env!("OUT_DIR"),
    "/bundled-decks/bundled_decks_generated.rs"
));

#[derive(Debug, Clone, Copy)]
struct EmbeddedBundledDeck {
    package_id: &'static str,
    package_version: &'static str,
    archive_sha256: &'static str,
    bytes: &'static [u8],
}

fn embedded_bundled_decks() -> &'static [EmbeddedBundledDeck] {
    static DECKS: [EmbeddedBundledDeck; 2] = [
        EmbeddedBundledDeck {
            package_id: "org.latentdeck.deck.d2",
            package_version: "0.2.0",
            archive_sha256: D2_ARCHIVE_SHA256,
            bytes: D2_ARCHIVE_BYTES,
        },
        EmbeddedBundledDeck {
            package_id: "org.latentdeck.deck.q4",
            package_version: "0.2.0",
            archive_sha256: Q4_ARCHIVE_SHA256,
            bytes: Q4_ARCHIVE_BYTES,
        },
    ];
    &DECKS
}

fn bundled_index() -> Result<BundledPackageIndex, ExtensionError> {
    serde_json::from_str(BUNDLED_INDEX_JSON).map_err(|error| {
        ExtensionError::new(
            ErrorCode::ManifestInvalid,
            format!("build-generated bundled Deck index is invalid: {error}"),
        )
    })
}

fn deck_reference(deck: &EmbeddedBundledDeck) -> PackageReference {
    PackageReference {
        kind: PackageKind::DeckPack,
        package_id: deck.package_id.to_owned(),
        package_version: deck.package_version.to_owned(),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct BundledDeckProvisionReport {
    pub(crate) installed: Vec<PackageReference>,
    pub(crate) enabled: Vec<PackageReference>,
    pub(crate) preserved: Vec<PackageReference>,
    pub(crate) issues: Vec<BundledDeckProvisionIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BundledDeckProvisionIssue {
    pub(crate) package: PackageReference,
    pub(crate) code: ErrorCode,
}

/// Provision the two build-authorized Deck packages without changing an
/// already-installed exact version or selecting over another installed
/// version of the same Deck ID.
pub(crate) fn provision_bundled_decks(
    roots: &ExtensionRoots,
) -> Result<BundledDeckProvisionReport, ExtensionError> {
    let initial = list(roots)?;
    let index = bundled_index()?;
    let mut report = BundledDeckProvisionReport::default();
    for deck in embedded_bundled_decks() {
        let package = deck_reference(deck);
        if let Some(existing) = find_exact(&initial, deck) {
            match validate_existing_exact(roots, deck, existing) {
                Ok(()) => report.preserved.push(package),
                Err(error) => push_issue(&mut report, package, &error),
            }
            continue;
        }

        let another_version_exists = initial.iter().any(|candidate| {
            candidate.package.kind == PackageKind::DeckPack
                && candidate.package.package_id == deck.package_id
        });
        let temporary = match tempfile::Builder::new()
            .prefix("latentdeck-bundled-deck-")
            .tempdir()
        {
            Ok(temporary) => temporary,
            Err(error) => {
                push_issue(
                    &mut report,
                    package,
                    &io_error("create bundled Deck staging directory", &error),
                );
                continue;
            }
        };
        let archive_path = temporary.path().join("package.ld");
        if let Err(error) = fs::write(&archive_path, deck.bytes) {
            push_issue(
                &mut report,
                package,
                &io_error("write bundled Deck archive", &error),
            );
            continue;
        }
        let receipt = match install_from_bundled_index(
            roots,
            &InstallRequest {
                archive_path,
                expected_sha256: deck.archive_sha256.to_owned(),
            },
            &index,
        ) {
            Ok(receipt) => receipt,
            Err(error) => {
                push_issue(&mut report, package, &error);
                continue;
            }
        };
        if receipt.inspection.package != package
            || receipt.inspection.archive_sha256 != deck.archive_sha256
        {
            push_issue(
                &mut report,
                package,
                &ExtensionError::new(
                    ErrorCode::PackageUntrusted,
                    "bundled Deck installation did not preserve its build-authorized identity",
                ),
            );
            continue;
        }
        report.installed.push(package.clone());

        let after_install = list(roots)?;
        let Some(exact) = find_exact(&after_install, deck) else {
            push_issue(
                &mut report,
                package,
                &ExtensionError::new(
                    ErrorCode::PackageMissing,
                    "bundled Deck disappeared immediately after installation",
                ),
            );
            continue;
        };
        if let Err(error) = validate_existing_exact(roots, deck, exact) {
            push_issue(&mut report, package, &error);
            continue;
        }
        let same_id_count = after_install
            .iter()
            .filter(|candidate| {
                candidate.package.kind == PackageKind::DeckPack
                    && candidate.package.package_id == deck.package_id
            })
            .count();
        if !another_version_exists && same_id_count == 1 {
            match enable_if_only_installed_version(roots, &package) {
                Ok(_) => report.enabled.push(package),
                Err(error) => push_issue(&mut report, package, &error),
            }
        }
    }
    Ok(report)
}

fn find_exact<'a>(
    packages: &'a [InstalledPackageSummary],
    deck: &EmbeddedBundledDeck,
) -> Option<&'a InstalledPackageSummary> {
    packages
        .iter()
        .find(|candidate| candidate.package == deck_reference(deck))
}

fn validate_existing_exact(
    roots: &ExtensionRoots,
    deck: &EmbeddedBundledDeck,
    existing: &InstalledPackageSummary,
) -> Result<(), ExtensionError> {
    match &existing.health {
        PackageHealth::Healthy => {}
        PackageHealth::Corrupt => {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                format!(
                    "installed bundled Deck {}@{} is corrupt and will not be overwritten",
                    deck.package_id, deck.package_version
                ),
            ));
        }
        PackageHealth::Untrusted => {
            return Err(ExtensionError::new(
                ErrorCode::PackageUntrusted,
                format!(
                    "installed bundled Deck {}@{} is untrusted and will not be overwritten",
                    deck.package_id, deck.package_version
                ),
            ));
        }
    }
    let validated = resolve_installed(roots, &existing.package)?;
    if validated.trust_receipt().archive_sha256 != deck.archive_sha256 {
        return Err(ExtensionError::new(
            ErrorCode::PackageUntrusted,
            format!(
                "installed bundled Deck {}@{} has a different immutable archive hash",
                deck.package_id, deck.package_version
            ),
        ));
    }
    Ok(())
}

fn io_error(context: &str, error: &std::io::Error) -> ExtensionError {
    ExtensionError::new(ErrorCode::Io, format!("{context}: {error}"))
}

fn push_issue(
    report: &mut BundledDeckProvisionReport,
    package: PackageReference,
    error: &ExtensionError,
) {
    report.issues.push(BundledDeckProvisionIssue {
        package,
        code: error.code(),
    });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use latentdeck_extension_manager::{
        BundledPackageEntry, ExtensionRoots, PackRequest, PackageHealth, disable, enable, inspect,
        install_from_bundled_index, list, pack,
    };

    use super::*;

    #[test]
    fn embeds_exact_d2_and_q4_archives() {
        let decks = embedded_bundled_decks();
        assert_eq!(decks.len(), 2);
        assert_eq!(
            decks
                .iter()
                .map(|deck| (deck.package_id, deck.package_version))
                .collect::<Vec<_>>(),
            [
                ("org.latentdeck.deck.d2", "0.2.0"),
                ("org.latentdeck.deck.q4", "0.2.0"),
            ]
        );
        assert!(
            decks
                .iter()
                .all(|deck| { deck.archive_sha256.len() == 64 && !deck.bytes.is_empty() })
        );

        let index = bundled_index().expect("parse embedded index");
        assert_eq!(index.packages.len(), decks.len());
        for deck in decks {
            let temporary = tempfile::tempdir().expect("temporary bundled archive directory");
            let archive = temporary.path().join("bundled.ld");
            fs::write(&archive, deck.bytes).expect("write archive");
            let inspection = inspect(&archive, Some(deck.archive_sha256))
                .expect("embedded archive must inspect");
            assert_eq!(inspection.package, deck_reference(deck));
            assert_eq!(inspection.archive_sha256, deck.archive_sha256);
            assert!(index.packages.iter().any(|entry| {
                entry.package == inspection.package
                    && entry.archive_sha256 == inspection.archive_sha256
            }));
        }
    }

    #[test]
    fn embedded_archives_match_a_fresh_deterministic_pack_of_authoritative_sources() {
        for deck in embedded_bundled_decks() {
            let source_name = match deck.package_id {
                "org.latentdeck.deck.d2" => "d2",
                "org.latentdeck.deck.q4" => "q4",
                _ => panic!("unexpected bundled Deck identity"),
            };
            let temporary = tempfile::tempdir().expect("temporary deterministic pack directory");
            let receipt = pack(&PackRequest {
                source_directory: repository_root()
                    .join("operators/builtin")
                    .join(source_name)
                    .join("package"),
                output_path: temporary.path().join("fresh.ld"),
            })
            .expect("fresh deterministic bundled Deck pack");
            assert_eq!(receipt.inspection.package, deck_reference(deck));
            assert_eq!(receipt.inspection.archive_sha256, deck.archive_sha256);
        }
    }

    #[test]
    fn first_provision_installs_and_enables_exact_bundled_versions() {
        let temporary = tempfile::tempdir().expect("temporary extension roots");
        let roots = ExtensionRoots::for_base_root(temporary.path().join("LatentDeck"));

        let report = provision_bundled_decks(&roots).expect("first bundled provision");

        assert_eq!(report.installed.len(), 2);
        assert_eq!(report.enabled.len(), 2);
        assert!(report.preserved.is_empty());
        assert!(report.issues.is_empty());
        let packages = list(&roots).expect("list provisioned packages");
        assert_eq!(packages.len(), 2);
        for deck in embedded_bundled_decks() {
            let package = packages
                .iter()
                .find(|summary| summary.package == deck_reference(deck))
                .expect("exact bundled Deck is installed");
            assert_eq!(package.health, PackageHealth::Healthy);
            assert!(package.enabled);
        }
    }

    #[test]
    fn repeated_provision_preserves_exact_versions_and_does_not_reenable() {
        let temporary = tempfile::tempdir().expect("temporary extension roots");
        let roots = ExtensionRoots::for_base_root(temporary.path().join("LatentDeck"));
        provision_bundled_decks(&roots).expect("first bundled provision");
        let d2 = deck_reference(&embedded_bundled_decks()[0]);
        disable(&roots, &d2).expect("explicitly disable bundled D2");
        let receipt_path = roots
            .receipt_root(PackageKind::DeckPack)
            .join(&d2.package_id)
            .join(format!("{}.json", d2.package_version));
        let receipt_before = fs::read(&receipt_path).expect("read disabled trust receipt");

        let report = provision_bundled_decks(&roots).expect("repeated bundled provision");

        assert!(report.installed.is_empty());
        assert!(report.enabled.is_empty());
        assert_eq!(report.preserved.len(), 2);
        assert!(report.issues.is_empty());
        assert_eq!(
            fs::read(&receipt_path).expect("read preserved trust receipt"),
            receipt_before
        );
        let packages = list(&roots).expect("list preserved packages");
        assert!(
            !packages
                .iter()
                .find(|summary| summary.package == d2)
                .expect("D2 summary")
                .enabled
        );
    }

    #[test]
    fn alternative_active_version_prevents_automatic_bundled_activation() {
        let temporary = tempfile::tempdir().expect("temporary extension roots");
        let roots = ExtensionRoots::for_base_root(temporary.path().join("LatentDeck"));
        let source = temporary.path().join("alternate-d2-source");
        copy_tree(
            &repository_root().join("operators/builtin/d2/package"),
            &source,
        );
        let manifest_path = source.join("deck-pack.json");
        let mut manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(&manifest_path).expect("read alternate Deck manifest"),
        )
        .expect("parse alternate Deck manifest");
        manifest["deck_version"] = serde_json::Value::String("0.1.0".to_owned());
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("serialize alternate Deck manifest"),
        )
        .expect("write alternate Deck manifest");
        let archive = temporary.path().join("alternate-d2.ld");
        let packed = pack(&PackRequest {
            source_directory: source,
            output_path: archive.clone(),
        })
        .expect("pack alternate D2 fixture");
        let alternate = packed.inspection.package.clone();
        assert_eq!(alternate.package_version, "0.1.0");
        let test_index = BundledPackageIndex {
            index_version: "1.0.0".to_owned(),
            packages: vec![BundledPackageEntry {
                package: alternate.clone(),
                archive_sha256: packed.inspection.archive_sha256.clone(),
            }],
        };
        install_from_bundled_index(
            &roots,
            &InstallRequest {
                archive_path: archive,
                expected_sha256: packed.inspection.archive_sha256,
            },
            &test_index,
        )
        .expect("install authorized alternate D2 fixture");
        enable(&roots, &alternate).expect("activate alternate D2 fixture");

        let report = provision_bundled_decks(&roots).expect("provision bundled Decks");

        let bundled_d2 = deck_reference(&embedded_bundled_decks()[0]);
        assert!(report.installed.contains(&bundled_d2));
        assert!(!report.enabled.contains(&bundled_d2));
        assert!(report.issues.is_empty());
        let packages = list(&roots).expect("list Deck versions");
        assert!(
            packages
                .iter()
                .find(|summary| summary.package == alternate)
                .expect("alternate D2 remains installed")
                .enabled
        );
        assert!(
            !packages
                .iter()
                .find(|summary| summary.package == bundled_d2)
                .expect("bundled D2 is installed")
                .enabled
        );
    }

    #[test]
    fn corrupt_exact_bundled_install_fails_closed_without_repair() {
        let temporary = tempfile::tempdir().expect("temporary extension roots");
        let roots = ExtensionRoots::for_base_root(temporary.path().join("LatentDeck"));
        provision_bundled_decks(&roots).expect("first bundled provision");
        let d2 = deck_reference(&embedded_bundled_decks()[0]);
        let operator_path = roots
            .decks_root
            .join(&d2.package_id)
            .join(&d2.package_version)
            .join("operator.json");
        let mut tampered = fs::read(&operator_path).expect("read installed operator descriptor");
        tampered.push(b' ');
        fs::write(&operator_path, &tampered).expect("tamper installed operator descriptor");

        let report = provision_bundled_decks(&roots).expect("isolate corrupt exact version");

        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].package, d2);
        assert_eq!(report.issues[0].code, ErrorCode::IntegrityFailed);
        assert_eq!(report.preserved.len(), 1);
        assert_eq!(
            fs::read(&operator_path).expect("read untouched corrupt descriptor"),
            tampered
        );
        let packages = list(&roots).expect("list isolated corrupt package");
        assert_eq!(
            packages
                .iter()
                .find(|summary| summary.package == d2)
                .expect("corrupt D2 summary")
                .health,
            PackageHealth::Corrupt
        );
    }

    fn repository_root() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
    }

    fn copy_tree(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).expect("create copied package directory");
        for entry in fs::read_dir(source).expect("read package source directory") {
            let entry = entry.expect("read package source entry");
            let target = destination.join(entry.file_name());
            let file_type = entry.file_type().expect("read package source file type");
            if file_type.is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                assert!(file_type.is_file(), "package fixture contains only files");
                fs::copy(entry.path(), target).expect("copy package source file");
            }
        }
    }
}
