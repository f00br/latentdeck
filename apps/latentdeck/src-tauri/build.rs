use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use latentdeck_extension_manager::{
    BundledPackageEntry, BundledPackageIndex, PackRequest, PackageKind, PackageReference, inspect,
    pack,
};

const D2_ID: &str = "org.latentdeck.deck.d2";
const Q4_ID: &str = "org.latentdeck.deck.q4";
const BUNDLED_VERSION: &str = "0.2.0";

fn main() {
    build_bundled_decks().expect("build deterministic bundled D2/Q4 Deck packages");
    tauri_build::build();
}

fn build_bundled_decks() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or("CARGO_MANIFEST_DIR is unavailable while building bundled Deck packages")?,
    );
    let repository_root = manifest_dir.join("../../..");
    let d2_source = repository_root.join("operators/builtin/d2/package");
    let q4_source = repository_root.join("operators/builtin/q4/package");
    emit_tree_reruns(&d2_source)?;
    emit_tree_reruns(&q4_source)?;

    let output_root = PathBuf::from(
        std::env::var_os("OUT_DIR")
            .ok_or("OUT_DIR is unavailable while building bundled Deck packages")?,
    )
    .join("bundled-decks");
    fs::create_dir_all(&output_root)?;

    let d2_name = format!("{D2_ID}-{BUNDLED_VERSION}.ld");
    let q4_name = format!("{Q4_ID}-{BUNDLED_VERSION}.ld");
    let d2 = pack_exact(&d2_source, &output_root.join(&d2_name), D2_ID)?;
    let q4 = pack_exact(&q4_source, &output_root.join(&q4_name), Q4_ID)?;

    let index = BundledPackageIndex {
        index_version: "1.0.0".to_owned(),
        packages: vec![
            BundledPackageEntry {
                package: d2.package.clone(),
                archive_sha256: d2.archive_sha256.clone(),
            },
            BundledPackageEntry {
                package: q4.package.clone(),
                archive_sha256: q4.archive_sha256.clone(),
            },
        ],
    };
    let mut index_json = serde_json::to_vec(&index)?;
    index_json.push(b'\n');
    fs::write(output_root.join("bundled-decks-index.json"), index_json)?;

    let generated = format!(
        concat!(
            "pub(super) const D2_ARCHIVE_BYTES: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/bundled-decks/{d2_name}\"));\n",
            "pub(super) const Q4_ARCHIVE_BYTES: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/bundled-decks/{q4_name}\"));\n",
            "pub(super) const D2_ARCHIVE_SHA256: &str = \"{d2_sha256}\";\n",
            "pub(super) const Q4_ARCHIVE_SHA256: &str = \"{q4_sha256}\";\n",
            "pub(super) const BUNDLED_INDEX_JSON: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/bundled-decks/bundled-decks-index.json\"));\n",
        ),
        d2_name = d2_name,
        q4_name = q4_name,
        d2_sha256 = d2.archive_sha256,
        q4_sha256 = q4.archive_sha256,
    );
    fs::write(output_root.join("bundled_decks_generated.rs"), generated)?;
    Ok(())
}

fn pack_exact(
    source: &Path,
    output: &Path,
    expected_id: &str,
) -> Result<latentdeck_extension_manager::InspectedPackage, Box<dyn Error>> {
    if output.try_exists()? {
        fs::remove_file(output)?;
    }
    let receipt = pack(&PackRequest {
        source_directory: source.to_owned(),
        output_path: output.to_owned(),
    })?;
    let inspection = inspect(output, Some(&receipt.inspection.archive_sha256))?;
    let expected = PackageReference {
        kind: PackageKind::DeckPack,
        package_id: expected_id.to_owned(),
        package_version: BUNDLED_VERSION.to_owned(),
    };
    if inspection.package != expected || receipt.inspection != inspection {
        return Err(format!(
            "bundled Deck source did not pack to exact identity {expected_id}@{BUNDLED_VERSION}"
        )
        .into());
    }
    Ok(inspection)
}

fn emit_tree_reruns(root: &Path) -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed={}", root.display());
    let mut entries = fs::read_dir(root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            emit_tree_reruns(&path)?;
        } else if file_type.is_file() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    Ok(())
}
