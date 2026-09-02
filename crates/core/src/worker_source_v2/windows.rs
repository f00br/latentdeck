//! Audited Win32 `DuplicateHandle` boundary for retained LC files.

#![allow(unsafe_code)]

use std::{
    ffi::c_void,
    io,
    os::windows::io::{AsRawHandle, BorrowedHandle},
    ptr,
};

use windows_sys::Win32::{
    Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE},
    System::Threading::GetCurrentProcess,
};

pub fn duplicate_into_process(
    source_file: BorrowedHandle<'_>,
    target_process: BorrowedHandle<'_>,
) -> io::Result<u64> {
    let mut duplicated: HANDLE = ptr::null_mut();
    // SAFETY: source/target handles remain borrowed and live for this call;
    // `duplicated` receives one target-process-owned handle value. No ownership
    // of either input handle is transferred.
    let succeeded = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            source_file.as_raw_handle().cast::<c_void>(),
            target_process.as_raw_handle().cast::<c_void>(),
            &raw mut duplicated,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if succeeded == 0 || duplicated.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(duplicated.addr() as u64)
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        io::{Read, Seek, SeekFrom, Write},
        os::windows::io::{AsHandle, BorrowedHandle, FromRawHandle, RawHandle},
    };

    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn duplicate_is_independently_owned_and_reads_the_same_file() {
        let mut source = NamedTempFile::new().expect("temporary source");
        source
            .write_all(b"retained-lc-handle")
            .expect("write source");
        source.as_file_mut().flush().expect("flush source");

        // SAFETY: GetCurrentProcess returns a live pseudo-handle for this
        // process and the borrow is confined to the synchronous duplication.
        let current_process = unsafe {
            BorrowedHandle::borrow_raw(GetCurrentProcess().cast::<c_void>() as RawHandle)
        };
        let duplicate_value = duplicate_into_process(source.as_file().as_handle(), current_process)
            .expect("duplicate into current process");
        let duplicate_address = usize::try_from(duplicate_value).expect("HANDLE fits usize");
        let duplicate_raw = std::ptr::with_exposed_provenance_mut::<c_void>(duplicate_address);
        // SAFETY: DuplicateHandle returned a new current-process-owned handle;
        // File assumes that exact ownership once and closes it on drop.
        let mut duplicate = unsafe { File::from_raw_handle(duplicate_raw) };

        drop(source);
        duplicate
            .seek(SeekFrom::Start(0))
            .expect("rewind duplicate");
        let mut bytes = Vec::new();
        duplicate.read_to_end(&mut bytes).expect("read duplicate");
        assert_eq!(bytes, b"retained-lc-handle");
    }
}
