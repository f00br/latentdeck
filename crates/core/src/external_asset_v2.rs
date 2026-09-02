//! Retained external Codec Pack asset validation for Protocol 2 sessions.
//!
//! Hashing a path and closing it leaves a replacement window before the
//! isolated worker opens the same asset.  This boundary hashes the exact file
//! handle and retains it without share-write or share-delete for the complete
//! Player or Deck session lifetime.

use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::Path,
    sync::Arc,
};

#[cfg(windows)]
use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
#[cfg(windows)]
use std::path::PathBuf;

use latentdeck_control::v2::ExternalAssetBinding;
use sha2::{Digest, Sha256};

#[cfg(windows)]
const FILE_SHARE_READ_ONLY: u32 = 0x0000_0001;
#[cfg(windows)]
const FILE_SHARE_READ_WRITE: u32 = 0x0000_0001 | 0x0000_0002;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

#[derive(Debug)]
pub enum RetainedExternalAssetError {
    Invalid,
    Io(std::io::Error),
}

impl From<std::io::Error> for RetainedExternalAssetError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Debug)]
pub struct IntegrityValidatedExternalAsset {
    binding: ExternalAssetBinding,
    _retained: Arc<RetainedExternalAssetFiles>,
}

#[derive(Debug)]
pub(crate) struct RetainedExternalAssetFiles {
    _file: File,
    _ancestor_directories: Vec<File>,
}

impl IntegrityValidatedExternalAsset {
    /// Hash and retain one exact external Codec asset without share-write or
    /// share-delete access.
    ///
    /// # Errors
    ///
    /// Rejects non-regular paths, length or digest mismatches, and I/O
    /// failures without returning the machine path in the error.
    pub fn validate_and_retain(
        binding: ExternalAssetBinding,
    ) -> Result<Self, RetainedExternalAssetError> {
        let file = retain_exact_external_asset(&binding)?;
        Ok(Self::from_validated_file(binding, file))
    }

    pub(crate) fn from_validated_file(
        binding: ExternalAssetBinding,
        retained: RetainedExternalAssetFiles,
    ) -> Self {
        Self {
            binding,
            _retained: Arc::new(retained),
        }
    }

    #[must_use]
    pub const fn binding(&self) -> &ExternalAssetBinding {
        &self.binding
    }

    /// Clone retained integrity evidence without reopening or rehashing the
    /// external asset.
    #[must_use]
    pub fn clone_retained(&self) -> Self {
        self.clone()
    }
}

pub(crate) fn retain_exact_external_asset(
    binding: &ExternalAssetBinding,
) -> Result<RetainedExternalAssetFiles, RetainedExternalAssetError> {
    let path = Path::new(&binding.path);
    if !path.is_absolute() {
        return Err(RetainedExternalAssetError::Invalid);
    }
    let ancestor_directories = retain_safe_ancestor_directories(path)?;
    let link_metadata = std::fs::symlink_metadata(path)?;
    if is_reparse_or_symlink(&link_metadata) || !link_metadata.is_file() {
        return Err(RetainedExternalAssetError::Invalid);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    options
        .share_mode(FILE_SHARE_READ_ONLY)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let mut file = options.open(path)?;
    let opened_metadata = file.metadata()?;
    if is_reparse_or_symlink(&opened_metadata)
        || !opened_metadata.is_file()
        || opened_metadata.len() != binding.byte_length
    {
        return Err(RetainedExternalAssetError::Invalid);
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if hex::encode(hasher.finalize()) != binding.sha256 {
        return Err(RetainedExternalAssetError::Invalid);
    }
    if file.metadata()?.len() != binding.byte_length {
        return Err(RetainedExternalAssetError::Invalid);
    }
    Ok(RetainedExternalAssetFiles {
        _file: file,
        _ancestor_directories: ancestor_directories,
    })
}

fn is_reparse_or_symlink(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn retain_safe_ancestor_directories(path: &Path) -> Result<Vec<File>, RetainedExternalAssetError> {
    let mut ancestors = Vec::<PathBuf>::new();
    let mut current = path.parent();
    while let Some(directory) = current {
        let metadata = std::fs::symlink_metadata(directory)?;
        if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
            return Err(RetainedExternalAssetError::Invalid);
        }
        // A filesystem root cannot be renamed out from under this path and a
        // no-delete handle there would be unnecessarily broad. Every mutable
        // descendant directory is pinned below.
        if directory.parent().is_none() {
            break;
        }
        ancestors.push(directory.to_path_buf());
        current = directory.parent();
    }
    ancestors.reverse();
    ancestors
        .into_iter()
        .map(|directory| {
            let mut options = OpenOptions::new();
            options
                .read(true)
                .share_mode(FILE_SHARE_READ_WRITE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
            let handle = options.open(&directory)?;
            let metadata = handle.metadata()?;
            if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
                return Err(RetainedExternalAssetError::Invalid);
            }
            Ok(handle)
        })
        .collect()
}

#[cfg(not(windows))]
fn retain_safe_ancestor_directories(_path: &Path) -> Result<Vec<File>, RetainedExternalAssetError> {
    Ok(Vec::new())
}

#[cfg(all(test, windows))]
mod tests {
    use std::{
        fs::OpenOptions,
        io::Write as _,
        os::windows::fs::{OpenOptionsExt as _, symlink_dir},
    };

    use latentdeck_control::v2::ExternalAssetBinding;
    use sha2::{Digest as _, Sha256};

    use super::retain_exact_external_asset;

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;

    #[test]
    fn exact_asset_handle_denies_share_write_and_delete_until_session_drop() {
        let directory = tempfile::tempdir().expect("temporary external asset directory");
        let path = directory.path().join("decoder.safetensors");
        let bytes = b"exact external codec asset";
        std::fs::write(&path, bytes).expect("write external asset");
        let binding = ExternalAssetBinding {
            asset_id: "decoder".to_owned(),
            path: path.to_string_lossy().into_owned(),
            sha256: hex::encode(Sha256::digest(bytes)),
            byte_length: u64::try_from(bytes.len()).expect("asset length"),
        };

        let retained = retain_exact_external_asset(&binding).expect("retain exact asset");

        let write_attempt = OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&path)
            .and_then(|mut file| file.write_all(b"mutate"));
        assert!(
            write_attempt.is_err(),
            "retained asset must deny a share-write open"
        );
        assert!(
            std::fs::remove_file(&path).is_err(),
            "retained asset must deny deletion"
        );

        drop(retained);
        std::fs::remove_file(&path).expect("asset becomes removable after session drop");
    }

    #[test]
    fn exact_asset_retains_ancestor_identity_until_session_drop() {
        let directory = tempfile::tempdir().expect("temporary external asset directory");
        let ancestor = directory.path().join("weights");
        std::fs::create_dir(&ancestor).expect("create weights directory");
        let path = ancestor.join("decoder.safetensors");
        let bytes = b"exact external codec asset";
        std::fs::write(&path, bytes).expect("write external asset");
        let binding = ExternalAssetBinding {
            asset_id: "decoder".to_owned(),
            path: path.to_string_lossy().into_owned(),
            sha256: hex::encode(Sha256::digest(bytes)),
            byte_length: u64::try_from(bytes.len()).expect("asset length"),
        };

        let retained = retain_exact_external_asset(&binding).expect("retain exact asset tree");
        let renamed = directory.path().join("weights-renamed");
        assert!(
            std::fs::rename(&ancestor, &renamed).is_err(),
            "retained ancestor must deny rename while the worker resolves the leaf path"
        );

        drop(retained);
        std::fs::rename(&ancestor, &renamed).expect("ancestor becomes renameable after drop");
    }

    #[test]
    fn external_asset_rejects_a_reparse_ancestor() {
        let directory = tempfile::tempdir().expect("temporary external asset directory");
        let real = directory.path().join("real-weights");
        std::fs::create_dir(&real).expect("create real weights directory");
        let path = real.join("decoder.safetensors");
        let bytes = b"exact external codec asset";
        std::fs::write(&path, bytes).expect("write external asset");
        let linked = directory.path().join("linked-weights");
        symlink_dir(&real, &linked).expect("create test directory symlink");
        let linked_path = linked.join("decoder.safetensors");
        let binding = ExternalAssetBinding {
            asset_id: "decoder".to_owned(),
            path: linked_path.to_string_lossy().into_owned(),
            sha256: hex::encode(Sha256::digest(bytes)),
            byte_length: u64::try_from(bytes.len()).expect("asset length"),
        };

        assert!(
            retain_exact_external_asset(&binding).is_err(),
            "a worker path through a replaceable reparse ancestor must fail closed"
        );
    }
}
