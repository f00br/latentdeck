use serde_json::Value;

use crate::error::{ErrorCode, ExtensionError, Result};
use crate::model::PackageKind;
use crate::schema::parse_strict_json;

#[derive(Clone, Copy)]
pub(crate) enum PublicSchema {
    DeckPack,
    Operator,
    Faceplate,
    CodecPack,
    Integrity,
}

impl PublicSchema {
    pub(crate) const fn for_manifest(kind: PackageKind) -> Self {
        match kind {
            PackageKind::DeckPack => Self::DeckPack,
            PackageKind::CodecPack => Self::CodecPack,
        }
    }

    const fn source(self) -> &'static str {
        match self {
            Self::DeckPack => include_str!("../../../spec/deck-package/deck-pack.schema.json"),
            Self::Operator => include_str!("../../../spec/deck-package/operator.schema.json"),
            Self::Faceplate => include_str!("../../../spec/deck-package/faceplate.schema.json"),
            Self::CodecPack => include_str!("../../../spec/codec-pack/codec-pack.schema.json"),
            Self::Integrity => {
                include_str!("../../../spec/extension-package/integrity.schema.json")
            }
        }
    }
}

pub(crate) fn validate_public_schema(
    bytes: &[u8],
    schema: PublicSchema,
    context: &str,
) -> Result<()> {
    let instance: Value = parse_strict_json(bytes, context)?;
    let schema_document: Value = serde_json::from_str(schema.source()).map_err(|error| {
        ExtensionError::new(
            ErrorCode::ManifestInvalid,
            format!("embedded public JSON Schema for {context} is invalid: {error}"),
        )
    })?;
    let validator = jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(&schema_document)
        .map_err(|error| {
            ExtensionError::new(
                ErrorCode::ManifestInvalid,
                format!("embedded public JSON Schema for {context} cannot compile: {error}"),
            )
        })?;
    validator.validate(&instance).map_err(|error| {
        ExtensionError::new(
            ErrorCode::ManifestInvalid,
            format!("{context} does not match its public JSON Schema: {error}"),
        )
    })
}
