//! Retained external Codec Pack asset validation for Protocol 2 sessions.
//!
//! Hashing a path and closing it leaves a replacement window before the
//! isolated worker opens the same asset.  This boundary hashes the exact file
//! handle and retains it without share-write or share-delete for the complete
//! Player or Deck session lifetime.

use std::{
    fs::{File, OpenOptions},
    io::Read,
    os::windows::fs::OpenOptionsExt as _,
    path::Path,
};

use latentdeck_control::v2::ExternalAssetBinding;
use sha2::{Digest, Sha256};

const FILE_SHARE_READ_ONLY: u32 = 0x0000_0001;

#[derive(Debug)]
pub(crate) enum RetainedExternalAssetError {
    Invalid,
    Io(std::io::Error),
}

impl From<std::io::Error> for RetainedExternalAssetError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub(crate) fn retain_exact_external_asset(
    binding: &ExternalAssetBinding,
) -> Result<File, RetainedExternalAssetError> {
    let path = Path::new(&binding.path);
    if !path.is_absolute() {
        return Err(RetainedExternalAssetError::Invalid);
    }
    let link_metadata = std::fs::symlink_metadata(path)?;
    if link_metadata.file_type().is_symlink() || !link_metadata.is_file() {
        return Err(RetainedExternalAssetError::Invalid);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ_ONLY)
        .open(path)?;
    if file.metadata()?.len() != binding.byte_length {
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
    Ok(file)
}

#[cfg(test)]
mod tests {
    use std::{fs::OpenOptions, io::Write as _, os::windows::fs::OpenOptionsExt as _};

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
}
