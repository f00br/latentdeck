//! Host-owned construction of Protocol 2 Deck runtime bindings.
//!
//! Only an enabled, revalidated package with a live usage lease can produce a
//! dynamic Deck load command. Callers cannot inject a Python path, entrypoint,
//! identity, or trust hash through [`DeckLoadRequest`].

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use latentdeck_control::v2::{
    Command, ControlBinding, ControlValue, DeckLoad, DeckRuntimeBinding, LimitedVec, RoleBinding,
    SourceBinding,
};
use latentdeck_extension_manager::{ActiveInstalledPackage, PackageKind, PackageManifest};
use semver::Version;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

const OPERATOR_SCHEMA_VERSION: &str = "0.2.0";
const DECK_OPERATOR_API_VERSION: &str = "0.2.0";
const MAX_OPERATOR_DESCRIPTOR_BYTES: u64 = 1024 * 1024;
const MAX_PATH_BYTES: usize = 32_768;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_CONTROL_TEXT_BYTES: usize = 4_096;
const MAX_SOURCES: usize = 16;
const MAX_ROLES: usize = 16;
const MAX_CONTROLS: usize = 64;
const MAX_ENUM_OPTIONS: usize = 256;
const MAX_EXACT_F64_INTEGER: i64 = 9_007_199_254_740_992;

/// Closed `operator.json` schema accepted by the Protocol 2 Deck host.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckOperatorDescriptor {
    pub schema_version: String,
    pub deck_operator_api: String,
    pub deck_id: String,
    pub deck_version: String,
    pub operator_id: String,
    pub operator_version: String,
    pub entrypoint: String,
    pub source_count: u8,
    pub role_ids: Vec<String>,
    pub controls: Vec<DeckOperatorControlDescriptor>,
}

/// One closed typed control declaration from `operator.json`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckOperatorControlDescriptor {
    pub control_id: String,
    pub value_type: DeckOperatorControlKind,
    pub default: Value,
    #[serde(default)]
    pub options: Option<Vec<String>>,
    #[serde(default)]
    pub minimum: Option<f64>,
    #[serde(default)]
    pub maximum: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeckOperatorControlKind {
    Boolean,
    Integer,
    Number,
    Enum,
    Text,
}

/// User/session state allowed to vary for one exact active Deck runtime.
///
/// Package identities, paths, entrypoints, and hashes are deliberately absent.
#[derive(Debug, Clone, PartialEq)]
pub struct DeckLoadRequest {
    pub deck_session_id: Uuid,
    pub sources: Vec<SourceBinding>,
    pub roles: Vec<RoleBinding>,
    pub controls: Vec<ControlBinding>,
    pub seed: u64,
    pub stream_generation: u64,
}

/// An exact active Deck runtime plus the shared package usage lease that keeps
/// its immutable installed tree alive for the complete worker session.
#[derive(Debug)]
pub struct ActiveDeckRuntime {
    active_package: ActiveInstalledPackage,
    operator: DeckOperatorDescriptor,
    binding: DeckRuntimeBinding,
}

impl ActiveDeckRuntime {
    /// Build the only trusted dynamic runtime representation from an active
    /// package lease.
    ///
    /// # Errors
    ///
    /// Returns a closed error if the package kind, receipt, canonical tree,
    /// operator descriptor, or manifest/operator cross-check is invalid.
    pub fn from_active_package(
        active_package: ActiveInstalledPackage,
    ) -> Result<Self, DeckRuntimeError> {
        let (operator, binding) = {
            let manifest = match active_package.manifest() {
                PackageManifest::Deck(manifest) => manifest,
                PackageManifest::Codec(_) => return Err(DeckRuntimeError::NotDeckPackage),
            };
            let receipt = active_package.trust_receipt();
            if !receipt.enabled
                || receipt.package.kind != PackageKind::DeckPack
                || receipt.package.package_id != manifest.deck_id
                || receipt.package.package_version != manifest.deck_version
                || !canonical_sha256(&receipt.manifest_sha256)
                || !canonical_sha256(&receipt.integrity_catalog_sha256)
            {
                return Err(DeckRuntimeError::TrustReceiptInvalid);
            }

            let package_root = canonical_directory(active_package.root())?;
            let descriptor_path = canonical_file(
                &package_root,
                &package_root.join(&manifest.runtime.operator_descriptor_path),
            )?;
            let metadata =
                fs::metadata(&descriptor_path).map_err(|_| DeckRuntimeError::PackageTreeInvalid)?;
            if metadata.len() == 0 || metadata.len() > MAX_OPERATOR_DESCRIPTOR_BYTES {
                return Err(DeckRuntimeError::OperatorDescriptorInvalid);
            }
            let descriptor_bytes = fs::read(&descriptor_path)
                .map_err(|_| DeckRuntimeError::OperatorDescriptorInvalid)?;
            if descriptor_bytes.is_empty()
                || u64::try_from(descriptor_bytes.len()).unwrap_or(u64::MAX)
                    > MAX_OPERATOR_DESCRIPTOR_BYTES
            {
                return Err(DeckRuntimeError::OperatorDescriptorInvalid);
            }
            let operator: DeckOperatorDescriptor = serde_json::from_slice(&descriptor_bytes)
                .map_err(|_| DeckRuntimeError::OperatorDescriptorInvalid)?;
            validate_operator(&operator)?;
            crosscheck_operator(manifest, &operator)?;

            let python_root =
                canonical_directory(&package_root.join(&manifest.runtime.python_root))?;
            if !python_root.starts_with(&package_root) {
                return Err(DeckRuntimeError::PackageTreeInvalid);
            }
            let python_root = bounded_absolute_path(&python_root)?;
            let binding = DeckRuntimeBinding {
                deck_id: manifest.deck_id.clone(),
                deck_version: manifest.deck_version.clone(),
                operator_id: operator.operator_id.clone(),
                operator_version: operator.operator_version.clone(),
                python_root,
                entrypoint: manifest.runtime.entrypoint.clone(),
                package_manifest_sha256: receipt.manifest_sha256.clone(),
                integrity_catalog_sha256: receipt.integrity_catalog_sha256.clone(),
            };
            (operator, binding)
        };

        Ok(Self {
            active_package,
            operator,
            binding,
        })
    }

    #[must_use]
    pub const fn operator_descriptor(&self) -> &DeckOperatorDescriptor {
        &self.operator
    }

    #[must_use]
    pub const fn runtime_binding(&self) -> &DeckRuntimeBinding {
        &self.binding
    }

    /// Retain visibility of the exact leased package without exposing any
    /// constructor that accepts an arbitrary local tree.
    #[must_use]
    pub const fn active_package(&self) -> &ActiveInstalledPackage {
        &self.active_package
    }

    /// Construct a typed `deck.load` command bound to this exact package.
    ///
    /// # Errors
    ///
    /// Returns an error when sources, roles, or controls do not match the
    /// closed operator descriptor.
    pub fn build_load_command(
        &self,
        request: DeckLoadRequest,
    ) -> Result<Command, DeckRuntimeError> {
        validate_load_request(&self.operator, &request)?;
        let sources = LimitedVec::<SourceBinding, MAX_SOURCES>::try_from_vec(request.sources)
            .map_err(|_| DeckRuntimeError::LoadRequestInvalid("sources"))?;
        let roles = LimitedVec::<RoleBinding, MAX_ROLES>::try_from_vec(request.roles)
            .map_err(|_| DeckRuntimeError::LoadRequestInvalid("roles"))?;
        let controls = LimitedVec::<ControlBinding, MAX_CONTROLS>::try_from_vec(request.controls)
            .map_err(|_| DeckRuntimeError::LoadRequestInvalid("controls"))?;
        Ok(Command::DeckLoad(Box::new(DeckLoad {
            deck_session_id: request.deck_session_id,
            deck_id: self.binding.deck_id.clone(),
            deck_version: self.binding.deck_version.clone(),
            operator_id: self.binding.operator_id.clone(),
            operator_version: self.binding.operator_version.clone(),
            runtime: Some(self.binding.clone()),
            sources,
            roles,
            controls,
            seed: request.seed,
            stream_generation: request.stream_generation,
        })))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DeckRuntimeError {
    #[error("active package is not a Deck package")]
    NotDeckPackage,
    #[error("active Deck trust receipt is invalid")]
    TrustReceiptInvalid,
    #[error("active Deck package tree is invalid")]
    PackageTreeInvalid,
    #[error("operator.json does not match its closed schema")]
    OperatorDescriptorInvalid,
    #[error("operator.json does not match deck-pack.json: {0}")]
    ManifestOperatorMismatch(&'static str),
    #[error("Deck load request does not match operator.json: {0}")]
    LoadRequestInvalid(&'static str),
}

fn canonical_directory(path: &Path) -> Result<PathBuf, DeckRuntimeError> {
    let canonical = fs::canonicalize(path).map_err(|_| DeckRuntimeError::PackageTreeInvalid)?;
    if !canonical.is_absolute() || !canonical.is_dir() {
        return Err(DeckRuntimeError::PackageTreeInvalid);
    }
    Ok(canonical)
}

fn canonical_file(package_root: &Path, path: &Path) -> Result<PathBuf, DeckRuntimeError> {
    let canonical = fs::canonicalize(path).map_err(|_| DeckRuntimeError::PackageTreeInvalid)?;
    if !canonical.starts_with(package_root) || !canonical.is_file() {
        return Err(DeckRuntimeError::PackageTreeInvalid);
    }
    Ok(canonical)
}

fn bounded_absolute_path(path: &Path) -> Result<String, DeckRuntimeError> {
    let path = path.to_str().ok_or(DeckRuntimeError::PackageTreeInvalid)?;
    if !Path::new(path).is_absolute() || path.is_empty() || path.len() > MAX_PATH_BYTES {
        return Err(DeckRuntimeError::PackageTreeInvalid);
    }
    Ok(path.to_owned())
}

fn canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_operator(operator: &DeckOperatorDescriptor) -> Result<(), DeckRuntimeError> {
    if operator.schema_version != OPERATOR_SCHEMA_VERSION
        || operator.deck_operator_api != DECK_OPERATOR_API_VERSION
        || operator.source_count == 0
        || usize::from(operator.source_count) > MAX_SOURCES
        || operator.role_ids.len() != usize::from(operator.source_count)
        || operator.controls.len() > MAX_CONTROLS
    {
        return Err(DeckRuntimeError::OperatorDescriptorInvalid);
    }
    identifier(&operator.deck_id)?;
    version(&operator.deck_version)?;
    identifier(&operator.operator_id)?;
    version(&operator.operator_version)?;
    python_entrypoint(&operator.entrypoint)?;

    let mut roles = HashSet::new();
    for role in &operator.role_ids {
        identifier(role)?;
        if !roles.insert(role.as_str()) {
            return Err(DeckRuntimeError::OperatorDescriptorInvalid);
        }
    }
    let mut controls = HashSet::new();
    for control in &operator.controls {
        validate_control_descriptor(control)?;
        if !controls.insert(control.control_id.as_str()) {
            return Err(DeckRuntimeError::OperatorDescriptorInvalid);
        }
    }
    Ok(())
}

fn crosscheck_operator(
    manifest: &latentdeck_extension_manager::DeckPackManifest,
    operator: &DeckOperatorDescriptor,
) -> Result<(), DeckRuntimeError> {
    if operator.deck_id != manifest.deck_id {
        return Err(DeckRuntimeError::ManifestOperatorMismatch("deck_id"));
    }
    if operator.deck_version != manifest.deck_version {
        return Err(DeckRuntimeError::ManifestOperatorMismatch("deck_version"));
    }
    if operator.entrypoint != manifest.runtime.entrypoint {
        return Err(DeckRuntimeError::ManifestOperatorMismatch("entrypoint"));
    }
    if operator.source_count != manifest.signal.slots {
        return Err(DeckRuntimeError::ManifestOperatorMismatch("source_count"));
    }
    let manifest_roles: Vec<&str> = manifest
        .signal
        .roles
        .iter()
        .map(|role| role.role_id.as_str())
        .collect();
    let operator_roles: Vec<&str> = operator.role_ids.iter().map(String::as_str).collect();
    if operator_roles != manifest_roles {
        return Err(DeckRuntimeError::ManifestOperatorMismatch("role_ids"));
    }
    Ok(())
}

fn validate_control_descriptor(
    control: &DeckOperatorControlDescriptor,
) -> Result<(), DeckRuntimeError> {
    identifier(&control.control_id)?;
    let has_range =
        control.minimum.is_some() || control.maximum.is_some() || control.step.is_some();
    match control.value_type {
        DeckOperatorControlKind::Boolean => {
            if !control.default.is_boolean() || control.options.is_some() || has_range {
                return Err(DeckRuntimeError::OperatorDescriptorInvalid);
            }
        }
        DeckOperatorControlKind::Integer => {
            let default = exact_integer_as_f64(json_integer(&control.default)?)?;
            validate_numeric_bounds(control, default, true)?;
        }
        DeckOperatorControlKind::Number => {
            let default = control
                .default
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or(DeckRuntimeError::OperatorDescriptorInvalid)?;
            validate_numeric_bounds(control, default, false)?;
        }
        DeckOperatorControlKind::Enum => {
            let default = control
                .default
                .as_str()
                .ok_or(DeckRuntimeError::OperatorDescriptorInvalid)?;
            let options = control
                .options
                .as_ref()
                .filter(|values| !values.is_empty() && values.len() <= MAX_ENUM_OPTIONS)
                .ok_or(DeckRuntimeError::OperatorDescriptorInvalid)?;
            if has_range
                || options.iter().any(|value| !bounded_text(value))
                || !options.iter().any(|value| value == default)
            {
                return Err(DeckRuntimeError::OperatorDescriptorInvalid);
            }
            let unique: HashSet<_> = options.iter().collect();
            if unique.len() != options.len() {
                return Err(DeckRuntimeError::OperatorDescriptorInvalid);
            }
        }
        DeckOperatorControlKind::Text => {
            if control
                .default
                .as_str()
                .is_none_or(|value| !bounded_text(value))
                || control.options.is_some()
                || has_range
            {
                return Err(DeckRuntimeError::OperatorDescriptorInvalid);
            }
        }
    }
    Ok(())
}

fn validate_numeric_bounds(
    control: &DeckOperatorControlDescriptor,
    default: f64,
    integral: bool,
) -> Result<(), DeckRuntimeError> {
    if control.options.is_some()
        || [control.minimum, control.maximum, control.step]
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite() || (integral && value.fract() != 0.0))
        || control.step.is_some_and(|value| value <= 0.0)
        || control
            .minimum
            .zip(control.maximum)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
        || control.minimum.is_some_and(|minimum| default < minimum)
        || control.maximum.is_some_and(|maximum| default > maximum)
    {
        return Err(DeckRuntimeError::OperatorDescriptorInvalid);
    }
    Ok(())
}

fn json_integer(value: &Value) -> Result<i64, DeckRuntimeError> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .ok_or(DeckRuntimeError::OperatorDescriptorInvalid)
}

fn exact_integer_as_f64(value: i64) -> Result<f64, DeckRuntimeError> {
    if !(-MAX_EXACT_F64_INTEGER..=MAX_EXACT_F64_INTEGER).contains(&value) {
        return Err(DeckRuntimeError::OperatorDescriptorInvalid);
    }
    value
        .to_string()
        .parse::<f64>()
        .map_err(|_| DeckRuntimeError::OperatorDescriptorInvalid)
}

fn identifier(value: &str) -> Result<(), DeckRuntimeError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(DeckRuntimeError::OperatorDescriptorInvalid);
    }
    Ok(())
}

fn python_entrypoint(value: &str) -> Result<(), DeckRuntimeError> {
    let Some((module, callable)) = value.split_once(':') else {
        return Err(DeckRuntimeError::OperatorDescriptorInvalid);
    };
    if callable.contains(':')
        || module.is_empty()
        || callable.is_empty()
        || !module.split('.').all(python_identifier)
        || !python_identifier(callable)
    {
        return Err(DeckRuntimeError::OperatorDescriptorInvalid);
    }
    Ok(())
}

fn python_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn version(value: &str) -> Result<(), DeckRuntimeError> {
    Version::parse(value)
        .map(|_| ())
        .map_err(|_| DeckRuntimeError::OperatorDescriptorInvalid)
}

fn bounded_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_CONTROL_TEXT_BYTES && !value.contains('\0')
}

fn validate_load_request(
    operator: &DeckOperatorDescriptor,
    request: &DeckLoadRequest,
) -> Result<(), DeckRuntimeError> {
    if request.deck_session_id.is_nil()
        || request.stream_generation == 0
        || request.sources.len() != usize::from(operator.source_count)
        || request.roles.len() != operator.role_ids.len()
        || request.controls.len() > operator.controls.len()
    {
        return Err(DeckRuntimeError::LoadRequestInvalid("shape"));
    }
    let mut source_slots: Vec<u8> = request
        .sources
        .iter()
        .map(|source| source.physical_slot)
        .collect();
    source_slots.sort_unstable();
    if source_slots != (1..=operator.source_count).collect::<Vec<_>>() {
        return Err(DeckRuntimeError::LoadRequestInvalid("sources"));
    }
    let role_ids: HashSet<&str> = request
        .roles
        .iter()
        .map(|role| role.role.as_str())
        .collect();
    let expected_roles: HashSet<&str> = operator.role_ids.iter().map(String::as_str).collect();
    let role_slots: HashSet<u8> = request
        .roles
        .iter()
        .map(|role| role.physical_slot)
        .collect();
    if role_ids != expected_roles
        || role_slots.len() != request.roles.len()
        || request.roles.iter().any(|role| {
            role.physical_slot == 0 || usize::from(role.physical_slot) > request.sources.len()
        })
    {
        return Err(DeckRuntimeError::LoadRequestInvalid("roles"));
    }

    let descriptors: std::collections::HashMap<&str, &DeckOperatorControlDescriptor> = operator
        .controls
        .iter()
        .map(|control| (control.control_id.as_str(), control))
        .collect();
    let mut seen = HashSet::new();
    for control in &request.controls {
        if !seen.insert(control.name.as_str()) {
            return Err(DeckRuntimeError::LoadRequestInvalid("controls"));
        }
        let descriptor = descriptors
            .get(control.name.as_str())
            .ok_or(DeckRuntimeError::LoadRequestInvalid("controls"))?;
        validate_control_binding(descriptor, &control.value)?;
    }
    Ok(())
}

fn validate_control_binding(
    descriptor: &DeckOperatorControlDescriptor,
    value: &ControlValue,
) -> Result<(), DeckRuntimeError> {
    let valid = match (descriptor.value_type, value) {
        (DeckOperatorControlKind::Boolean, ControlValue::Boolean(_)) => true,
        (DeckOperatorControlKind::Integer, ControlValue::Integer(value)) => {
            exact_integer_as_f64(*value)
                .is_ok_and(|value| numeric_value_in_bounds(descriptor, value))
        }
        (DeckOperatorControlKind::Number, ControlValue::Number(value)) => {
            value.is_finite() && numeric_value_in_bounds(descriptor, *value)
        }
        (DeckOperatorControlKind::Enum, ControlValue::Text(value)) => descriptor
            .options
            .as_ref()
            .is_some_and(|options| options.contains(value)),
        (DeckOperatorControlKind::Text, ControlValue::Text(value)) => bounded_text(value),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(DeckRuntimeError::LoadRequestInvalid("controls"))
    }
}

fn numeric_value_in_bounds(descriptor: &DeckOperatorControlDescriptor, value: f64) -> bool {
    descriptor.minimum.is_none_or(|minimum| value >= minimum)
        && descriptor.maximum.is_none_or(|maximum| value <= maximum)
}
