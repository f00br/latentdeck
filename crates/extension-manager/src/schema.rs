use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use semver::Version;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::{Map, Number, Value};

use crate::error::{ErrorCode, ExtensionError, Result};
use crate::model::{
    BundledPackageIndex, CodecCapability, CodecPackManifest, DeckPackManifest, IntegrityCatalog,
    PackageKind, PackageManifest, ProfileKey, SignalGeometry, TimingDescriptor,
};

pub(crate) const DECK_MANIFEST_VERSION: &str = "1.0.0";
pub(crate) const CODEC_MANIFEST_VERSION: &str = "2.0.0";
pub(crate) const INTEGRITY_MANIFEST_VERSION: &str = "1.0.0";
pub(crate) const TRUST_RECEIPT_VERSION: &str = "1.0.0";
pub(crate) const BUNDLED_INDEX_VERSION: &str = "1.0.0";
pub(crate) const MAX_JSON_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_DECK_FILES: usize = 256;
pub(crate) const MAX_DECK_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_DECK_EXTRACTED_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_DECK_FILE_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_CODEC_FILES: usize = 32_768;
pub(crate) const MAX_CODEC_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024 * 1024;
pub(crate) const MAX_CODEC_EXTRACTED_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub(crate) const MAX_EXTERNAL_ASSETS: usize = 16;
pub(crate) const MAX_DECK_GEOMETRIES: usize = 64;
const MAX_PACKAGE_VERSION_BYTES: usize = 115;

struct StrictJson(Value);

impl<'de> Deserialize<'de> for StrictJson {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StrictVisitor;

        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = Value;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("JSON without duplicate object keys")
            }

            fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
                Ok(Value::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
                Ok(Value::Number(Number::from(value)))
            }

            fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
                Ok(Value::Number(Number::from(value)))
            }

            fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Number::from_f64(value)
                    .map(Value::Number)
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Value::String(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
                Ok(Value::String(value))
            }

            fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
                Ok(Value::Null)
            }

            fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
                Ok(Value::Null)
            }

            fn visit_some<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                StrictJson::deserialize(deserializer).map(|value| value.0)
            }

            fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<StrictJson>()? {
                    values.push(value.0);
                }
                Ok(Value::Array(values))
            }

            fn visit_map<A>(self, mut object: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = Map::new();
                while let Some((key, value)) = object.next_entry::<String, StrictJson>()? {
                    if values.insert(key.clone(), value.0).is_some() {
                        return Err(de::Error::custom(format!(
                            "duplicate JSON object key {key:?}"
                        )));
                    }
                }
                Ok(Value::Object(values))
            }
        }

        deserializer.deserialize_any(StrictVisitor).map(Self)
    }
}

pub(crate) fn parse_strict_json<T>(bytes: &[u8], context: &str) -> Result<T>
where
    T: DeserializeOwned + Serialize,
{
    parse_strict_json_with_limit(bytes, context, MAX_JSON_BYTES)
}

pub(crate) fn parse_strict_json_with_limit<T>(
    bytes: &[u8],
    context: &str,
    max_bytes: usize,
) -> Result<T>
where
    T: DeserializeOwned + Serialize,
{
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(ExtensionError::new(
            ErrorCode::ManifestInvalid,
            format!("{context} is empty or exceeds {max_bytes} bytes"),
        ));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let strict = StrictJson::deserialize(&mut deserializer).map_err(|error| {
        ExtensionError::new(
            ErrorCode::ManifestInvalid,
            format!("{context} is not strict JSON: {error}"),
        )
    })?;
    deserializer.end().map_err(|error| {
        ExtensionError::new(
            ErrorCode::ManifestInvalid,
            format!("{context} has trailing JSON data: {error}"),
        )
    })?;
    let value: T = serde_json::from_value(strict.0.clone()).map_err(|error| {
        ExtensionError::new(
            ErrorCode::ManifestInvalid,
            format!("{context} does not match its closed schema: {error}"),
        )
    })?;
    Ok(value)
}

pub(crate) fn canonical_json<T: Serialize>(value: &T, context: &str) -> Result<Vec<u8>> {
    serde_jcs::to_vec(value).map_err(|error| {
        ExtensionError::new(
            ErrorCode::ManifestInvalid,
            format!("{context} cannot be canonicalized: {error}"),
        )
    })
}

pub(crate) fn validate_strict_json_value(bytes: &[u8], context: &str) -> Result<()> {
    let _: Value = parse_strict_json(bytes, context)?;
    Ok(())
}

pub(crate) fn parse_manifest(kind: PackageKind, bytes: &[u8]) -> Result<PackageManifest> {
    let manifest = match kind {
        PackageKind::DeckPack => PackageManifest::Deck(parse_strict_json::<DeckPackManifest>(
            bytes,
            "deck-pack.json",
        )?),
        PackageKind::CodecPack => PackageManifest::Codec(parse_strict_json::<CodecPackManifest>(
            bytes,
            "codec-pack.json",
        )?),
    };
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub(crate) fn parse_integrity_catalog(bytes: &[u8], kind: PackageKind) -> Result<IntegrityCatalog> {
    let catalog: IntegrityCatalog = parse_strict_json(bytes, "integrity.json")?;
    if catalog.manifest_version != INTEGRITY_MANIFEST_VERSION {
        return Err(invalid("integrity.json manifest_version must be 1.0.0"));
    }
    let max_files = max_files(kind);
    if catalog.files.is_empty() || catalog.files.len().saturating_add(2) > max_files {
        return Err(invalid(format!(
            "integrity.json file count must leave room for both control files within {max_files} files"
        )));
    }
    let mut previous: Option<&str> = None;
    let mut total = 0_u64;
    for file in &catalog.files {
        validate_portable_relative_path(&file.path, false)?;
        if file.path == kind.manifest_name() || file.path == "integrity.json" {
            return Err(invalid(
                "integrity.json must not recursively catalog either control file",
            ));
        }
        if let Some(previous) = previous
            && previous >= file.path.as_str()
        {
            return Err(invalid(
                "integrity.json files must be unique and sorted by path",
            ));
        }
        validate_sha256(&file.sha256, "integrity file SHA-256")?;
        if kind == PackageKind::DeckPack && file.byte_length > MAX_DECK_FILE_BYTES {
            return Err(invalid("a .ld file exceeds the 1 MiB per-file bound"));
        }
        total = total
            .checked_add(file.byte_length)
            .ok_or_else(|| invalid("integrity file lengths overflow u64"))?;
        previous = Some(&file.path);
    }
    let control_allowance = 2 * u64::try_from(MAX_JSON_BYTES).expect("JSON bound fits u64");
    let maximum = max_extracted_bytes(kind);
    if total > maximum.saturating_sub(control_allowance) {
        return Err(invalid(
            "integrity file lengths exceed the extracted-size bound",
        ));
    }
    Ok(catalog)
}

fn validate_manifest(manifest: &PackageManifest) -> Result<()> {
    match manifest {
        PackageManifest::Deck(manifest) => validate_deck_manifest(manifest),
        PackageManifest::Codec(manifest) => validate_codec_manifest(manifest),
    }
}

fn validate_deck_manifest(manifest: &DeckPackManifest) -> Result<()> {
    if manifest.manifest_version != DECK_MANIFEST_VERSION || manifest.kind != PackageKind::DeckPack
    {
        return Err(invalid(
            "deck manifest must declare manifest_version 1.0.0 and kind deck_pack",
        ));
    }
    validate_reverse_dns_id(&manifest.deck_id, "deck_id")?;
    validate_package_version(&manifest.deck_version, "deck_version")?;
    validate_common_manifest_text(
        &manifest.display_name,
        &manifest.summary,
        &manifest.publisher.name,
        manifest.publisher.url.as_deref(),
        &manifest.license.spdx_or_label,
        &manifest.license.notice_path,
    )?;
    validate_version_range(
        &manifest.compatibility.app_min_inclusive,
        &manifest.compatibility.app_max_exclusive,
        "Deck app compatibility",
    )?;
    if manifest.compatibility.deck_host_api == 0
        || manifest.compatibility.worker_protocol == 0
        || manifest.compatibility.deck_operator_api == 0
    {
        return Err(invalid(
            "Deck compatibility API and protocol versions must be nonzero",
        ));
    }
    validate_runtime_constraints(
        &manifest.compatibility.tensor_abi,
        &manifest.compatibility.python.version,
        &manifest.compatibility.python.platform_tag,
        &manifest.compatibility.torch_exact_build,
    )?;
    if manifest.runtime.operator_descriptor_path != "operator.json"
        || manifest.faceplate_path != "faceplate.json"
    {
        return Err(invalid(
            "Deck packages must use operator.json and faceplate.json at the archive root",
        ));
    }
    validate_portable_relative_path(&manifest.runtime.python_root, true)?;
    validate_entrypoint(&manifest.runtime.entrypoint, "Deck runtime entrypoint")?;
    validate_deck_signal(manifest)?;
    validate_integrity_descriptor(
        manifest.integrity.catalog_path.as_str(),
        &manifest.integrity.catalog_sha256,
    )
}

fn validate_deck_signal(manifest: &DeckPackManifest) -> Result<()> {
    let signal = &manifest.signal;
    if !(1..=16).contains(&signal.slots)
        || signal.roles.len() != usize::from(signal.slots)
        || signal.default_permutation.len() != usize::from(signal.slots)
    {
        return Err(invalid(
            "Deck signal must declare 1-16 slots with one role and permutation item per slot",
        ));
    }
    let mut roles = BTreeSet::new();
    for role in &signal.roles {
        validate_local_id(&role.role_id, "role_id")?;
        validate_bounded_text(&role.display_name, "role display name", 1, 80)?;
        if !roles.insert(role.role_id.as_str()) {
            return Err(invalid("Deck roles must be unique"));
        }
    }
    let permutation: BTreeSet<&str> = signal
        .default_permutation
        .iter()
        .map(String::as_str)
        .collect();
    if permutation.len() != signal.default_permutation.len() || permutation != roles {
        return Err(invalid(
            "default_permutation must contain every role exactly once",
        ));
    }
    if !roles.contains(signal.structural_carrier_role.as_str()) {
        return Err(invalid("structural_carrier_role is not a declared role"));
    }
    if signal.geometry_allowlist.is_empty() || signal.geometry_allowlist.len() > MAX_DECK_GEOMETRIES
    {
        return Err(invalid(
            "geometry_allowlist must contain 1-64 exact geometries",
        ));
    }
    let mut geometries = HashSet::new();
    for geometry in &signal.geometry_allowlist {
        validate_geometry(geometry)?;
        if !geometries.insert(geometry) {
            return Err(invalid("geometry_allowlist contains a duplicate geometry"));
        }
    }
    validate_timing(&signal.timing)?;
    validate_capability_set(&signal.required_capabilities, false)?;
    if signal.required_capabilities.is_empty() {
        return Err(invalid("Deck required_capabilities must not be empty"));
    }
    if let Some(profiles) = &signal.profile_allowlist {
        if profiles.is_empty() || profiles.len() > 64 {
            return Err(invalid("profile_allowlist must contain 1-64 profiles"));
        }
        let mut seen = BTreeSet::new();
        for profile in profiles {
            validate_profile_key(profile)?;
            if !seen.insert(profile) {
                return Err(invalid("profile_allowlist contains a duplicate ProfileKey"));
            }
        }
    }
    Ok(())
}

fn validate_codec_manifest(manifest: &CodecPackManifest) -> Result<()> {
    if manifest.manifest_version != CODEC_MANIFEST_VERSION
        || manifest.kind != PackageKind::CodecPack
    {
        return Err(invalid(
            "codec manifest must declare manifest_version 2.0.0 and kind codec_pack",
        ));
    }
    validate_reverse_dns_id(&manifest.pack_id, "pack_id")?;
    validate_package_version(&manifest.pack_version, "pack_version")?;
    validate_common_manifest_text(
        &manifest.display_name,
        &manifest.summary,
        &manifest.publisher.name,
        manifest.publisher.url.as_deref(),
        &manifest.license.spdx_or_label,
        &manifest.license.notice_path,
    )?;
    validate_version_range(
        &manifest.compatibility.app_min_inclusive,
        &manifest.compatibility.app_max_exclusive,
        "Codec app compatibility",
    )?;
    if manifest.compatibility.worker_protocol == 0 || manifest.compatibility.codec_adapter_api == 0
    {
        return Err(invalid(
            "Codec compatibility API and protocol versions must be nonzero",
        ));
    }
    validate_runtime_constraints(
        &manifest.compatibility.tensor_abi,
        &manifest.compatibility.python.version,
        &manifest.compatibility.python.platform_tag,
        &manifest.compatibility.torch_exact_build,
    )?;
    if manifest.compatibility.lc_spec_versions.is_empty()
        || manifest.compatibility.lc_spec_versions.len() > 16
    {
        return Err(invalid("Codec lc_spec_versions must contain 1-16 values"));
    }
    let mut lc_versions = HashSet::new();
    for version in &manifest.compatibility.lc_spec_versions {
        validate_semver(version, "LC spec version")?;
        if !lc_versions.insert(version) {
            return Err(invalid("Codec lc_spec_versions contains a duplicate"));
        }
    }
    if manifest.compatibility.profiles.is_empty() || manifest.compatibility.profiles.len() > 64 {
        return Err(invalid("Codec profiles must contain 1-64 exact profiles"));
    }
    let mut profiles = BTreeSet::new();
    for profile in &manifest.compatibility.profiles {
        validate_profile_key(profile)?;
        if !profiles.insert(profile) {
            return Err(invalid("Codec profiles contains a duplicate ProfileKey"));
        }
    }
    validate_reverse_dns_id(&manifest.adapter.adapter_id, "adapter_id")?;
    if is_reserved_package_id(&manifest.adapter.adapter_id)
        && !is_reserved_package_id(&manifest.pack_id)
    {
        return Err(invalid(
            "the reserved org.latentdeck.* adapter namespace requires a reserved pack_id",
        ));
    }
    validate_semver(&manifest.adapter.adapter_version, "adapter_version")?;
    validate_entrypoint(&manifest.adapter.entrypoint, "Codec adapter entrypoint")?;
    validate_codec_worker(manifest)?;
    validate_capability_set(&manifest.capabilities, true)?;
    validate_external_assets(manifest)?;
    validate_portable_relative_path(&manifest.runtime_lock.path, false)?;
    validate_sha256(&manifest.runtime_lock.sha256, "runtime lock SHA-256")?;
    validate_integrity_descriptor(
        manifest.integrity.catalog_path.as_str(),
        &manifest.integrity.catalog_sha256,
    )
}

fn validate_codec_worker(manifest: &CodecPackManifest) -> Result<()> {
    validate_portable_relative_path(&manifest.worker.executable, false)?;
    validate_portable_relative_path(&manifest.worker.working_directory, true)?;
    if manifest.worker.arguments.len() > 64 {
        return Err(invalid("Codec worker arguments exceeds 64 items"));
    }
    for argument in &manifest.worker.arguments {
        validate_bounded_text(argument, "Codec worker argument", 0, 1024)?;
        if argument.contains('\0') || argument.contains(['\r', '\n']) {
            return Err(invalid(
                "Codec worker arguments cannot contain control separators",
            ));
        }
    }
    if !(100..=600_000).contains(&manifest.worker.start_timeout_ms)
        || !(100..=600_000).contains(&manifest.worker.heartbeat_timeout_ms)
    {
        return Err(invalid(
            "Codec worker timeouts must be between 100 and 600000 ms",
        ));
    }
    Ok(())
}

fn validate_external_assets(manifest: &CodecPackManifest) -> Result<()> {
    if manifest.external_assets.len() > MAX_EXTERNAL_ASSETS {
        return Err(invalid("Codec pack declares more than 16 external assets"));
    }
    let mut asset_ids = HashSet::new();
    for asset in &manifest.external_assets {
        validate_local_id(&asset.asset_id, "external asset id")?;
        if !asset_ids.insert(asset.asset_id.as_str()) {
            return Err(invalid("Codec external asset IDs must be unique"));
        }
        validate_bounded_text(&asset.display_name, "external asset display name", 1, 120)?;
        if asset.byte_length == 0 {
            return Err(invalid("external asset byte_length must be positive"));
        }
        validate_sha256(&asset.sha256, "external asset SHA-256")?;
        validate_optional_https_url(asset.source_url.as_deref(), "external asset source URL")?;
        validate_bounded_text(&asset.license_label, "external asset license", 1, 120)?;
        validate_optional_https_url(asset.license_url.as_deref(), "external asset license URL")?;
    }
    Ok(())
}

fn validate_common_manifest_text(
    display_name: &str,
    summary: &str,
    publisher_name: &str,
    publisher_url: Option<&str>,
    license: &str,
    notice_path: &str,
) -> Result<()> {
    validate_bounded_text(display_name, "display_name", 1, 120)?;
    validate_bounded_text(summary, "summary", 1, 500)?;
    validate_bounded_text(publisher_name, "publisher name", 1, 120)?;
    validate_optional_https_url(publisher_url, "publisher URL")?;
    validate_bounded_text(license, "license label", 1, 120)?;
    validate_portable_relative_path(notice_path, false).map(|_| ())
}

fn validate_integrity_descriptor(path: &str, sha256: &str) -> Result<()> {
    if path != "integrity.json" {
        return Err(invalid("integrity catalog_path must be integrity.json"));
    }
    validate_sha256(sha256, "integrity catalog SHA-256")
}

fn validate_runtime_constraints(
    tensor_abi: &str,
    python_version: &str,
    platform_tag: &str,
    torch_exact_build: &str,
) -> Result<()> {
    validate_contract_name(tensor_abi, "tensor_abi")?;
    validate_python_version(python_version)?;
    validate_local_id(platform_tag, "Python platform tag")?;
    if platform_tag.eq_ignore_ascii_case("any") {
        return Err(invalid(
            "Python platform tag must be an explicit closed identifier",
        ));
    }
    validate_semver(torch_exact_build, "exact Torch build")
}

fn validate_contract_name(value: &str, name: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || value == "*"
        || value.eq_ignore_ascii_case("any")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(invalid(format!(
            "{name} is not a bounded explicit contract identifier"
        )));
    }
    Ok(())
}

fn validate_python_version(value: &str) -> Result<()> {
    validate_bounded_text(value, "Python version", 3, 32)?;
    let parts = value.split('.').collect::<Vec<_>>();
    if !(2..=3).contains(&parts.len())
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(invalid(
            "Python version must contain two or three numeric components",
        ));
    }
    Ok(())
}

fn validate_capability_set(capabilities: &[CodecCapability], require_v2: bool) -> Result<()> {
    if capabilities.len() > 6 {
        return Err(invalid("capability list exceeds the closed capability set"));
    }
    let mut seen = HashSet::new();
    for capability in capabilities {
        if !seen.insert(*capability) {
            return Err(invalid("capability list contains a duplicate"));
        }
    }
    if require_v2 {
        for required in [
            CodecCapability::Player,
            CodecCapability::Realtime,
            CodecCapability::Resample,
            CodecCapability::SnapshotCapture,
            CodecCapability::LiveCapture,
        ] {
            if !seen.contains(&required) {
                return Err(invalid(
                    "Codec Pack v2 is missing a mandatory lifecycle capability",
                ));
            }
        }
    }
    Ok(())
}

fn validate_geometry(geometry: &SignalGeometry) -> Result<()> {
    if geometry.batch != 1
        || geometry.temporal != 1
        || geometry.channels == 0
        || geometry.channels > 16_384
        || geometry.height == 0
        || geometry.width == 0
        || geometry.height > 65_535
        || geometry.width > 65_535
    {
        return Err(invalid(
            "signal geometry must be finite [1,C,1,H,W] within closed bounds",
        ));
    }
    Ok(())
}

fn validate_timing(timing: &TimingDescriptor) -> Result<()> {
    if timing.frames_per_second_numerator == 0
        || timing.frames_per_second_denominator == 0
        || timing.frames_per_second_numerator > 1_000_000
        || timing.frames_per_second_denominator > 1_000_000
        || timing.samples_per_slot == 0
        || timing.samples_per_slot > 1_000_000
    {
        return Err(invalid("signal timing is outside closed positive bounds"));
    }
    Ok(())
}

fn validate_profile_key(profile: &ProfileKey) -> Result<()> {
    validate_local_id(&profile.codec_family, "codec family")?;
    validate_local_id(&profile.profile, "profile")?;
    if profile.codec_family.eq_ignore_ascii_case("any")
        || profile.profile.eq_ignore_ascii_case("any")
    {
        return Err(invalid("ProfileKey must not use an any constraint"));
    }
    validate_semver(&profile.profile_version, "profile version")
}

pub(crate) fn validate_package_reference(package: &crate::model::PackageReference) -> Result<()> {
    validate_reverse_dns_id(&package.package_id, "package_id")?;
    validate_package_version(&package.package_version, "package_version")
}

pub(crate) fn validate_bundled_index(index: &BundledPackageIndex) -> Result<()> {
    if index.index_version != BUNDLED_INDEX_VERSION {
        return Err(invalid("bundled package index version is unsupported"));
    }
    if index.packages.is_empty() || index.packages.len() > 256 {
        return Err(invalid(
            "bundled package index must contain between 1 and 256 entries",
        ));
    }
    let mut identities = BTreeSet::new();
    for entry in &index.packages {
        validate_package_reference(&entry.package)?;
        if !is_reserved_package_id(&entry.package.package_id) {
            return Err(invalid(
                "bundled package index may authorize only org.latentdeck.* identities",
            ));
        }
        validate_sha256(&entry.archive_sha256, "bundled archive SHA-256")?;
        if !identities.insert(&entry.package) {
            return Err(invalid(
                "bundled package index contains a duplicate identity",
            ));
        }
    }
    Ok(())
}

pub(crate) fn is_reserved_package_id(package_id: &str) -> bool {
    package_id == "org.latentdeck" || package_id.starts_with("org.latentdeck.")
}

pub(crate) fn validate_sha256(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(format!(
            "{name} must be canonical lowercase hexadecimal"
        )));
    }
    Ok(())
}

fn validate_semver(value: &str, name: &str) -> Result<()> {
    let parsed = Version::parse(value).map_err(|_| invalid(format!("{name} is not SemVer")))?;
    if parsed.to_string() != value {
        return Err(invalid(format!("{name} must be canonical SemVer")));
    }
    Ok(())
}

fn validate_package_version(value: &str, name: &str) -> Result<()> {
    validate_semver(value, name)?;
    // The version is both a directory component and a `<version>.json` receipt
    // component. Keeping the generated receipt name within the same 120-byte
    // portable-component bound also leaves one injective spelling on Windows.
    if value.len() > MAX_PACKAGE_VERSION_BYTES {
        return Err(invalid(format!(
            "{name} exceeds the {MAX_PACKAGE_VERSION_BYTES} byte storage-key bound"
        )));
    }
    if value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(invalid(format!(
            "{name} must be lowercase for an injective Windows storage key"
        )));
    }
    Ok(())
}

fn validate_version_range(minimum: &str, maximum: &str, name: &str) -> Result<()> {
    validate_semver(minimum, &format!("{name} minimum"))?;
    validate_semver(maximum, &format!("{name} maximum"))?;
    if Version::parse(minimum).expect("validated") >= Version::parse(maximum).expect("validated") {
        return Err(invalid(format!("{name} range is empty")));
    }
    Ok(())
}

fn validate_reverse_dns_id(value: &str, name: &str) -> Result<()> {
    if value.len() > 160 || !value.contains('.') {
        return Err(invalid(format!("{name} must be a bounded reverse-DNS ID")));
    }
    for segment in value.split('.') {
        if segment.is_empty()
            || segment.len() > 63
            || segment.starts_with('-')
            || segment.ends_with('-')
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(invalid(format!(
                "{name} must be a lowercase reverse-DNS ID"
            )));
        }
    }
    Ok(())
}

fn validate_local_id(value: &str, name: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 80
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(invalid(format!(
            "{name} is not a bounded lowercase identifier"
        )));
    }
    Ok(())
}

fn validate_entrypoint(value: &str, name: &str) -> Result<()> {
    if value.len() > 200 {
        return Err(invalid(format!("{name} is too long")));
    }
    let Some((module, callable)) = value.split_once(':') else {
        return Err(invalid(format!("{name} must use module:callable syntax")));
    };
    if module.is_empty()
        || callable.is_empty()
        || module.starts_with('.')
        || module.ends_with('.')
        || !module
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
        || !callable
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(invalid(format!(
            "{name} is not a portable Python entrypoint"
        )));
    }
    Ok(())
}

fn validate_bounded_text(value: &str, name: &str, minimum: usize, maximum: usize) -> Result<()> {
    if value.len() < minimum
        || value.len() > maximum
        || value.contains('\0')
        || value.contains(['\r', '\n'])
    {
        return Err(invalid(format!("{name} is outside its text bound")));
    }
    Ok(())
}

fn validate_optional_https_url(value: Option<&str>, name: &str) -> Result<()> {
    if let Some(value) = value {
        validate_bounded_text(value, name, 1, 500)?;
        if !value.starts_with("https://") {
            return Err(invalid(format!("{name} must use https")));
        }
    }
    Ok(())
}

pub(crate) fn validate_portable_relative_path(
    value: &str,
    allow_directory: bool,
) -> Result<PathBuf> {
    if value.is_empty()
        || value.len() > 240
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains('\\')
        || value.contains('\0')
    {
        return Err(invalid(
            "package path is not a bounded forward-slash relative path",
        ));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid("package path contains a non-normal component"));
    }
    let components: Vec<&str> = value.split('/').collect();
    for component in &components {
        validate_windows_component(component)?;
    }
    if !allow_directory && value.ends_with("/.") {
        return Err(invalid("file path ends with a directory marker"));
    }
    Ok(components.iter().collect())
}

fn validate_windows_component(component: &str) -> Result<()> {
    if component.is_empty()
        || component.len() > 120
        || component == "."
        || component == ".."
        || component.ends_with(['.', ' '])
        || !component.is_ascii()
        || component.bytes().any(|byte| {
            byte < 0x20
                || byte == 0x7f
                || matches!(byte, b'<' | b'>' | b':' | b'"' | b'|' | b'?' | b'*')
        })
    {
        return Err(invalid("package path contains an unsafe Windows component"));
    }
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end_matches(['.', ' ']);
    let upper = stem.to_ascii_uppercase();
    let reserved = matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || (upper.len() == 4
        && (upper.starts_with("COM") || upper.starts_with("LPT"))
        && upper.as_bytes()[3].is_ascii_digit()
        && upper.as_bytes()[3] != b'0');
    if reserved {
        return Err(invalid("package path uses a reserved Windows device name"));
    }
    Ok(())
}

pub(crate) fn validate_deck_file_extension(path: &str) -> Result<()> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension == "typed"
        && Path::new(path).file_name().and_then(|value| value.to_str()) != Some("py.typed")
    {
        return Err(invalid(format!(
            ".ld contains forbidden file type at {path}"
        )));
    }
    if !matches!(
        extension.as_str(),
        "py" | "typed" | "json" | "txt" | "md" | "png"
    ) {
        return Err(invalid(format!(
            ".ld contains forbidden file type at {path}"
        )));
    }
    Ok(())
}

pub(crate) const fn max_files(kind: PackageKind) -> usize {
    match kind {
        PackageKind::DeckPack => MAX_DECK_FILES,
        PackageKind::CodecPack => MAX_CODEC_FILES,
    }
}

pub(crate) const fn max_archive_bytes(kind: PackageKind) -> u64 {
    match kind {
        PackageKind::DeckPack => MAX_DECK_ARCHIVE_BYTES,
        PackageKind::CodecPack => MAX_CODEC_ARCHIVE_BYTES,
    }
}

pub(crate) const fn max_extracted_bytes(kind: PackageKind) -> u64 {
    match kind {
        PackageKind::DeckPack => MAX_DECK_EXTRACTED_BYTES,
        PackageKind::CodecPack => MAX_CODEC_EXTRACTED_BYTES,
    }
}

fn invalid(detail: impl Into<String>) -> ExtensionError {
    ExtensionError::new(ErrorCode::ManifestInvalid, detail)
}
