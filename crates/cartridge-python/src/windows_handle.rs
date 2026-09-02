//! Audited ownership conversion for a Core-duplicated Windows file handle.

#![allow(unsafe_code)]

use std::{
    fs::File,
    os::windows::io::{FromRawHandle, RawHandle},
};

/// Consume one owned handle value transferred into this process.
///
/// # Safety contract
///
/// The authenticated Core process created `raw_value` with `DuplicateHandle`
/// for this worker and transferred ownership exactly once.
pub fn consume_owned_file(raw_value: usize) -> File {
    // SAFETY: the caller validates nonzero/width and the authenticated P2
    // transport is the sole producer and consumer of the transferred handle.
    unsafe { File::from_raw_handle(raw_value as RawHandle) }
}
