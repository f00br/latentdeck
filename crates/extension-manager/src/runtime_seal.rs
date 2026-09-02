//! Persistent Windows validation seals for large installed Codec trees.
//!
//! A seal never replaces the integrity catalog. It records kernel-backed file
//! identities and NTFS change-journal positions observed while the catalogued
//! bytes were fully hashed and pinned. A later process may skip rereading those
//! bytes only when the exact trust receipt, closed tree, volume journal, and
//! every retained object stamp still agree.

use std::fs::File;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::model::{PackageReference, TrustReceipt};
use crate::schema::{canonical_json, parse_strict_json_with_limit};

const RUNTIME_SEAL_VERSION: &str = "1.0.0";
pub(crate) const MAX_RUNTIME_SEAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_SEALED_OBJECTS: usize = 131_072;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeValidationSeal {
    seal_version: String,
    package: PackageReference,
    trust_receipt_core_sha256: String,
    archive_sha256: String,
    manifest_sha256: String,
    integrity_catalog_sha256: String,
    tree: TreeStamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeStamp {
    volume_serial_number: String,
    usn_journal_id: String,
    objects: Vec<ObjectStamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectStamp {
    file_id: String,
    usn: String,
    byte_length: u64,
    file_attributes: u32,
    creation_time: String,
    last_write_time: String,
}

pub(crate) struct EncodedRuntimeSeal {
    pub bytes: Vec<u8>,
    pub sha256: String,
}

pub(crate) fn build(
    package: &PackageReference,
    receipt: &TrustReceipt,
    destination: &Path,
    object_handles: &[&File],
) -> Result<Option<EncodedRuntimeSeal>> {
    let Some(tree) = capture_tree_stamp(destination, object_handles) else {
        return Ok(None);
    };
    let seal = RuntimeValidationSeal {
        seal_version: RUNTIME_SEAL_VERSION.to_owned(),
        package: package.clone(),
        trust_receipt_core_sha256: receipt_core_sha256(receipt)?,
        archive_sha256: receipt.archive_sha256.clone(),
        manifest_sha256: receipt.manifest_sha256.clone(),
        integrity_catalog_sha256: receipt.integrity_catalog_sha256.clone(),
        tree,
    };
    let bytes = canonical_json(&seal, "runtime validation seal")?;
    if bytes.len() > MAX_RUNTIME_SEAL_BYTES {
        return Ok(None);
    }
    Ok(Some(EncodedRuntimeSeal {
        sha256: sha256(&bytes),
        bytes,
    }))
}

pub(crate) fn matches(
    bytes: &[u8],
    expected_sha256: &str,
    package: &PackageReference,
    receipt: &TrustReceipt,
    destination: &Path,
    object_handles: &[&File],
) -> Result<bool> {
    if bytes.len() > MAX_RUNTIME_SEAL_BYTES || sha256(bytes) != expected_sha256 {
        return Ok(false);
    }
    let seal: RuntimeValidationSeal = match parse_strict_json_with_limit(
        bytes,
        "runtime validation seal",
        MAX_RUNTIME_SEAL_BYTES,
    ) {
        Ok(seal) => seal,
        Err(_) => return Ok(false),
    };
    let receipt_core_sha256 = receipt_core_sha256(receipt)?;
    if seal.seal_version != RUNTIME_SEAL_VERSION
        || seal.package != *package
        || seal.trust_receipt_core_sha256 != receipt_core_sha256
        || seal.archive_sha256 != receipt.archive_sha256
        || seal.manifest_sha256 != receipt.manifest_sha256
        || seal.integrity_catalog_sha256 != receipt.integrity_catalog_sha256
        || seal.tree.objects.len() > MAX_SEALED_OBJECTS
        || seal.tree.objects.len() != object_handles.len()
    {
        return Ok(false);
    }
    Ok(capture_tree_stamp(destination, object_handles).is_some_and(|tree| tree == seal.tree))
}

fn receipt_core_sha256(receipt: &TrustReceipt) -> Result<String> {
    let mut core = receipt.clone();
    core.runtime_seal_sha256 = None;
    canonical_json(&core, "runtime trust receipt core").map(|bytes| sha256(&bytes))
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(not(windows))]
fn capture_tree_stamp(_destination: &Path, _object_handles: &[&File]) -> Option<TreeStamp> {
    None
}

#[cfg(windows)]
fn capture_tree_stamp(destination: &Path, object_handles: &[&File]) -> Option<TreeStamp> {
    windows::capture_tree_stamp(destination, object_handles)
}

#[cfg(windows)]
mod windows {
    #![allow(unsafe_code)]

    use std::ffi::c_void;
    use std::fs::{File, OpenOptions};
    use std::mem::{size_of, zeroed};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::{Component, Path, Prefix};
    use std::ptr;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo,
        GetFileInformationByHandleEx,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{
        FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_FILE_USN_DATA, READ_FILE_USN_DATA,
    };

    use super::{MAX_SEALED_OBJECTS, ObjectStamp, TreeStamp};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct UsnRecordV2Prefix {
        record_length: u32,
        major_version: u16,
        minor_version: u16,
        file_reference_number: u64,
        parent_file_reference_number: u64,
        usn: i64,
    }

    pub(super) fn capture_tree_stamp(
        destination: &Path,
        object_handles: &[&File],
    ) -> Option<TreeStamp> {
        if object_handles.is_empty() || object_handles.len() > MAX_SEALED_OBJECTS {
            return None;
        }
        let volume = open_volume(destination)?;
        let usn_journal_id = query_usn_journal_id(&volume)?;
        let mut objects = Vec::with_capacity(object_handles.len());
        let mut volume_serial_number = None;
        for file in object_handles {
            let stamp = stamp_file(file)?;
            match volume_serial_number {
                Some(expected) if expected != stamp.0 => return None,
                None => volume_serial_number = Some(stamp.0),
                Some(_) => {}
            }
            objects.push(stamp.1);
        }
        Some(TreeStamp {
            volume_serial_number: format!("{:016x}", volume_serial_number?),
            usn_journal_id: format!("{usn_journal_id:016x}"),
            objects,
        })
    }

    fn open_volume(path: &Path) -> Option<File> {
        let drive = match path.components().next()? {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => letter,
                _ => return None,
            },
            _ => return None,
        };
        let device = format!(r"\\.\{}:", char::from(drive));
        let mut options = OpenOptions::new();
        options
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
        options.open(device).ok()
    }

    fn query_usn_journal_id(volume: &File) -> Option<u64> {
        let mut output = [0_u8; 128];
        let mut returned = 0_u32;
        let succeeded = unsafe {
            DeviceIoControl(
                volume.as_raw_handle().cast::<c_void>(),
                FSCTL_QUERY_USN_JOURNAL,
                ptr::null(),
                0,
                output.as_mut_ptr().cast::<c_void>(),
                u32::try_from(output.len()).ok()?,
                &raw mut returned,
                ptr::null_mut(),
            )
        };
        if succeeded == 0 || returned < u32::try_from(size_of::<u64>()).ok()? {
            return None;
        }
        Some(u64::from_le_bytes(output[..8].try_into().ok()?))
    }

    fn stamp_file(file: &File) -> Option<(u64, ObjectStamp)> {
        let mut id_info: FILE_ID_INFO = unsafe { zeroed() };
        let succeeded = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle().cast::<c_void>(),
                FileIdInfo,
                (&raw mut id_info).cast::<c_void>(),
                u32::try_from(size_of::<FILE_ID_INFO>()).ok()?,
            )
        };
        if succeeded == 0 {
            return None;
        }
        let usn = read_file_usn(file)?;
        if usn < 0 {
            return None;
        }
        let metadata = file.metadata().ok()?;
        Some((
            id_info.VolumeSerialNumber,
            ObjectStamp {
                file_id: hex::encode(id_info.FileId.Identifier),
                usn: format!("{usn:016x}"),
                byte_length: metadata.file_size(),
                file_attributes: metadata.file_attributes(),
                creation_time: format!("{:016x}", metadata.creation_time()),
                last_write_time: format!("{:016x}", metadata.last_write_time()),
            },
        ))
    }

    fn read_file_usn(file: &File) -> Option<i64> {
        let input = READ_FILE_USN_DATA {
            MinMajorVersion: 2,
            MaxMajorVersion: 2,
        };
        let mut output = [0_u8; 1024];
        let mut returned = 0_u32;
        let succeeded = unsafe {
            DeviceIoControl(
                file.as_raw_handle().cast::<c_void>(),
                FSCTL_READ_FILE_USN_DATA,
                (&raw const input).cast::<c_void>(),
                u32::try_from(size_of::<READ_FILE_USN_DATA>()).ok()?,
                output.as_mut_ptr().cast::<c_void>(),
                u32::try_from(output.len()).ok()?,
                &raw mut returned,
                ptr::null_mut(),
            )
        };
        if succeeded == 0 || returned < u32::try_from(size_of::<UsnRecordV2Prefix>()).ok()? {
            return None;
        }
        let record = unsafe { ptr::read_unaligned(output.as_ptr().cast::<UsnRecordV2Prefix>()) };
        if record.major_version != 2
            || usize::try_from(record.record_length).ok()? > usize::try_from(returned).ok()?
        {
            return None;
        }
        Some(record.usn)
    }
}
