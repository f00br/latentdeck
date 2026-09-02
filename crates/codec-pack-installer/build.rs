use std::{
    collections::HashSet,
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use semver::Version;
use serde::Deserialize;

const AUTHORIZATION_FILE_ENV: &str = "LATENTDECK_H3_AUTHORIZATION_FILE";
const GENERATED_FILE: &str = "h3_authorization.rs";
const PACK_ID: &str = "org.latentdeck.h3";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizationFile {
    schema_version: u32,
    packages: Vec<AuthorizedPackage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizedPackage {
    pack_id: String,
    pack_version: String,
    archive_sha256: String,
    archive_byte_length: u64,
}

fn main() {
    println!("cargo:rerun-if-env-changed={AUTHORIZATION_FILE_ENV}");
    let packages = match env::var_os(AUTHORIZATION_FILE_ENV) {
        Some(path) => read_authorizations(Path::new(&path)),
        None => Vec::new(),
    };
    let generated = render_authorizations(&packages);
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo provides OUT_DIR")).join(GENERATED_FILE);
    fs::write(output, generated).expect("write generated H3 authorization");
}

fn read_authorizations(path: &Path) -> Vec<AuthorizedPackage> {
    println!("cargo:rerun-if-changed={}", path.display());
    let bytes = fs::read(path).expect("read LATENTDECK_H3_AUTHORIZATION_FILE");
    let file: AuthorizationFile =
        serde_json::from_slice(&bytes).expect("parse closed H3 authorization JSON");
    assert_eq!(
        file.schema_version, 1,
        "authorization schema_version must be 1"
    );
    assert!(
        (1..=16).contains(&file.packages.len()),
        "authorization must contain 1-16 packages"
    );
    let mut versions = HashSet::new();
    for package in &file.packages {
        assert_eq!(package.pack_id, PACK_ID, "authorization pack_id mismatch");
        Version::parse(&package.pack_version).expect("authorization pack_version must be SemVer");
        assert!(
            package.archive_sha256.len() == 64
                && package
                    .archive_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "authorization SHA-256 must be lowercase hexadecimal"
        );
        assert!(
            package.archive_byte_length > 0,
            "authorization archive length must be positive"
        );
        assert!(
            versions.insert(package.pack_version.as_str()),
            "authorization contains two archives for one immutable version"
        );
    }
    file.packages
}

fn render_authorizations(packages: &[AuthorizedPackage]) -> String {
    let mut output = String::from(
        "pub(crate) const EMBEDDED_H3_AUTHORIZATIONS: &[EmbeddedH3Authorization] = &[\n",
    );
    for package in packages {
        output.push_str("    EmbeddedH3Authorization {\n");
        writeln!(output, "        pack_version: {:?},", package.pack_version)
            .expect("write generated pack version");
        writeln!(
            output,
            "        archive_sha256: {:?},",
            package.archive_sha256
        )
        .expect("write generated archive hash");
        writeln!(
            output,
            "        archive_byte_length: {},",
            package.archive_byte_length
        )
        .expect("write generated archive length");
        output.push_str("    },\n");
    }
    output.push_str("];\n");
    output
}
