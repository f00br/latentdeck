//! Streaming SHA-256 helpers for cartridge identity and payload validation.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::error::{CartridgeError, ErrorCode, Result};

const SHA256_BYTES: usize = 32;
const SHA256_HEX_BYTES: usize = SHA256_BYTES * 2;
const HASH_BUFFER_BYTES: usize = 64 * 1024;

/// One canonical lower-case SHA-256 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256Hash([u8; SHA256_BYTES]);

impl Sha256Hash {
    /// Creates a digest from its exact 32-byte representation.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; SHA256_BYTES]) -> Self {
        Self(bytes)
    }

    /// Parses exactly 64 lower-case hexadecimal characters.
    ///
    /// # Errors
    ///
    /// Returns a manifest error when the digest is not canonical SHA-256 text.
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() != SHA256_HEX_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(CartridgeError::new(
                ErrorCode::ManifestInvalid,
                "SHA-256 must be 64 lowercase hexadecimal characters",
            ));
        }
        let mut bytes = [0_u8; SHA256_BYTES];
        hex::decode_to_slice(value, &mut bytes).map_err(|error| {
            CartridgeError::new(ErrorCode::ManifestInvalid, "SHA-256 text is invalid")
                .with_source(error)
        })?;
        Ok(Self(bytes))
    }

    /// Returns the digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; SHA256_BYTES] {
        &self.0
    }
}

impl fmt::Display for Sha256Hash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

impl FromStr for Sha256Hash {
    type Err = CartridgeError;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for Sha256Hash {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Sha256Hash {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Length and digest measured in one streaming pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MeasuredHash {
    pub byte_length: u64,
    pub sha256: Sha256Hash,
}

/// Hashes any reader without loading it into memory.
///
/// # Errors
///
/// Returns an I/O error if the stream cannot be read or its length overflows.
pub fn hash_reader<R: Read>(input: &mut R) -> Result<MeasuredHash> {
    let mut hasher = Sha256::new();
    let mut byte_length = 0_u64;
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = input.read(&mut buffer).map_err(|error| {
            CartridgeError::new(ErrorCode::IoRead, "cannot read data for SHA-256")
                .with_source(error)
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        byte_length = byte_length
            .checked_add(u64::try_from(read).map_err(|error| {
                CartridgeError::new(ErrorCode::RuntimeLimitExceeded, "read length exceeds u64")
                    .with_source(error)
            })?)
            .ok_or_else(|| {
                CartridgeError::new(
                    ErrorCode::RuntimeLimitExceeded,
                    "stream length overflows u64",
                )
            })?;
    }
    let bytes: [u8; SHA256_BYTES] = hasher.finalize().into();
    Ok(MeasuredHash {
        byte_length,
        sha256: Sha256Hash(bytes),
    })
}

/// Hashes a filesystem path using a bounded-memory stream.
///
/// # Errors
///
/// Returns an I/O error when the path cannot be opened or read.
pub fn hash_path(path: impl AsRef<Path>) -> Result<MeasuredHash> {
    let mut file = File::open(path.as_ref()).map_err(|error| {
        CartridgeError::new(ErrorCode::IoOpen, "cannot open path for SHA-256").with_source(error)
    })?;
    hash_reader(&mut file)
}
