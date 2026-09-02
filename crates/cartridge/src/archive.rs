//! Deterministic LC archive encoding and validation.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom, Write};

use crc32fast::Hasher;

use crate::error::{CartridgeError, ErrorCode, Result};
use crate::limits::{
    MAX_ARCHIVE_ENTRIES, MAX_H3_PAYLOAD_BYTES, MAX_MANIFEST_BYTES, MAX_PREVIEW_BYTES,
    MIN_ARCHIVE_ENTRIES,
};

const LOCAL_HEADER_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_HEADER_SIGNATURE: u32 = 0x0201_4b50;
const ZIP64_END_SIGNATURE: u32 = 0x0606_4b50;
const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
const END_SIGNATURE: u32 = 0x0605_4b50;
const ZIP64_EXTRA_ID: u16 = 0x0001;
const ZIP64_VERSION: u16 = 45;
const VERSION_MADE_BY: u16 = 0x032d;
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = 0x0021;
const ZIP64_END_BYTES: u64 = 56;
const ZIP64_LOCATOR_BYTES: u64 = 20;
const END_BYTES: u64 = 22;
const LOCAL_ZIP64_EXTRA_BYTES: u16 = 20;
const CENTRAL_ZIP64_EXTRA_BYTES: u16 = 28;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const COPY_BUFFER_BYTES_U64: u64 = 64 * 1024;

const MANIFEST_NAME: &str = "manifest.json";
const OPTIONAL_PREVIEW_NAME: &str = "preview.webp";

/// One source entry for deterministic archive creation.
pub struct EntryWrite<'a> {
    name: &'a str,
    size: u64,
    crc32: u32,
    reader: &'a mut dyn Read,
}

impl<'a> EntryWrite<'a> {
    /// Creates an entry whose declared length and checksum are verified while writing.
    pub fn new<R>(name: &'a str, size: u64, crc32: u32, reader: &'a mut R) -> Self
    where
        R: Read + 'a,
    {
        Self {
            name,
            size,
            crc32,
            reader,
        }
    }
}

/// A validated archive entry with an absolute data range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub name: String,
    pub size: u64,
    pub crc32: u32,
    pub data_offset: u64,
}

/// Structural result produced before manifest or tensor allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveIndex {
    pub entries: Vec<ArchiveEntry>,
    pub archive_size: u64,
}

#[derive(Debug, Clone)]
struct CentralEntry {
    name: String,
    size: u64,
    crc32: u32,
    local_offset: u64,
}

#[derive(Debug, Clone)]
struct WrittenEntry {
    name: String,
    size: u64,
    crc32: u32,
    local_offset: u64,
}

/// Calculates the ZIP CRC-32 used for an LC entry.
#[must_use]
pub fn payload_crc32(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

/// Writes the only archive representation accepted by LC 0.1.
///
/// # Errors
///
/// Returns an error when the requested entries violate the LC contract, a
/// source changes while being read, or the output cannot be written.
pub fn write_canonical<W>(output: &mut W, entries: &mut [EntryWrite<'_>]) -> Result<()>
where
    W: Write + Seek,
{
    validate_write_entries(entries)?;
    let initial_position = output
        .stream_position()
        .map_err(|error| write_error("cannot query output position", error))?;
    if initial_position != 0 {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveNoncanonical,
            "canonical archive output must begin at byte zero",
        ));
    }

    let mut written = Vec::with_capacity(entries.len());
    for entry in entries {
        let local_offset = output
            .stream_position()
            .map_err(|error| write_error("cannot query local header offset", error))?;
        write_local_header(output, entry.name, entry.size, entry.crc32)?;
        copy_entry(output, entry)?;
        written.push(WrittenEntry {
            name: entry.name.to_owned(),
            size: entry.size,
            crc32: entry.crc32,
            local_offset,
        });
    }

    let central_offset = output
        .stream_position()
        .map_err(|error| write_error("cannot query central directory offset", error))?;
    for entry in &written {
        write_central_header(output, entry)?;
    }
    let central_end = output
        .stream_position()
        .map_err(|error| write_error("cannot query central directory size", error))?;
    let central_size = central_end.checked_sub(central_offset).ok_or_else(|| {
        CartridgeError::new(ErrorCode::ArchiveMalformed, "central directory underflow")
    })?;
    let entry_count = u64::try_from(written.len()).map_err(|_| {
        CartridgeError::new(ErrorCode::EntryCountInvalid, "entry count does not fit u64")
    })?;
    write_zip64_terminator(output, entry_count, central_size, central_offset)?;
    output
        .flush()
        .map_err(|error| write_error("cannot flush canonical archive", error))
}

fn validate_write_entries(entries: &[EntryWrite<'_>]) -> Result<()> {
    if !(MIN_ARCHIVE_ENTRIES..=MAX_ARCHIVE_ENTRIES).contains(&entries.len()) {
        return Err(CartridgeError::new(
            ErrorCode::EntryCountInvalid,
            "LC 0.1 requires two entries and permits one optional preview",
        ));
    }

    for entry in entries {
        validate_safe_name(entry.name)?;
    }

    let mut folded_names = HashSet::with_capacity(entries.len());
    for entry in entries {
        if !folded_names.insert(entry.name.to_ascii_lowercase()) {
            return Err(CartridgeError::new(
                ErrorCode::EntryDuplicate,
                "archive entry names collide",
            )
            .at_entry(entry.name));
        }
    }

    validate_entry_order(entries.iter().map(|entry| entry.name))?;
    for entry in entries {
        validate_entry_size(entry.name, entry.size)?;
    }
    Ok(())
}

fn validate_entry_order<'a>(names: impl Iterator<Item = &'a str>) -> Result<()> {
    let names = names.collect::<Vec<_>>();
    let valid = match names.as_slice() {
        [manifest, payload] => *manifest == MANIFEST_NAME && is_payload_name(payload),
        [manifest, payload, preview] => {
            *manifest == MANIFEST_NAME
                && is_payload_name(payload)
                && *preview == OPTIONAL_PREVIEW_NAME
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(CartridgeError::new(
            ErrorCode::EntryUnexpected,
            "entries must use the canonical LC 0.1 names and order",
        ))
    }
}

fn validate_entry_size(name: &str, size: u64) -> Result<()> {
    let maximum = match name {
        MANIFEST_NAME => u64::try_from(MAX_MANIFEST_BYTES).expect("manifest limit fits u64"),
        OPTIONAL_PREVIEW_NAME => MAX_PREVIEW_BYTES,
        _ if is_payload_name(name) => MAX_H3_PAYLOAD_BYTES,
        _ => {
            return Err(CartridgeError::new(
                ErrorCode::EntryUnexpected,
                "entry is not part of LC 0.1",
            )
            .at_entry(name));
        }
    };
    if size > maximum {
        return Err(CartridgeError::new(
            ErrorCode::EntryTooLarge,
            "entry exceeds its immutable limit",
        )
        .at_entry(name));
    }
    Ok(())
}

fn is_payload_name(name: &str) -> bool {
    let Some(payload_id) = name
        .strip_prefix("payloads/")
        .and_then(|value| value.strip_suffix(".safetensors"))
    else {
        return false;
    };
    let length = payload_id.len();
    (1..=128).contains(&length)
        && payload_id.is_ascii()
        && payload_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
        && payload_id
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && payload_id
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

fn validate_safe_name(name: &str) -> Result<()> {
    let unsafe_name = name.is_empty()
        || !name.is_ascii()
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.ends_with('/')
        || name.contains('\\')
        || name.contains(':')
        || name
            .bytes()
            .any(|byte| byte == 0 || byte < b' ' || byte == 0x7f)
        || name
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..");
    if unsafe_name {
        return Err(
            CartridgeError::new(ErrorCode::EntryUnsafePath, "entry path is unsafe").at_entry(name),
        );
    }
    Ok(())
}

fn write_local_header<W: Write>(output: &mut W, name: &str, size: u64, crc32: u32) -> Result<()> {
    let name_length = name_length(name)?;
    write_u32(output, LOCAL_HEADER_SIGNATURE)?;
    write_u16(output, ZIP64_VERSION)?;
    write_u16(output, 0)?;
    write_u16(output, 0)?;
    write_u16(output, DOS_TIME)?;
    write_u16(output, DOS_DATE)?;
    write_u32(output, crc32)?;
    write_u32(output, u32::MAX)?;
    write_u32(output, u32::MAX)?;
    write_u16(output, name_length)?;
    write_u16(output, LOCAL_ZIP64_EXTRA_BYTES)?;
    write_all(output, name.as_bytes())?;
    write_u16(output, ZIP64_EXTRA_ID)?;
    write_u16(output, 16)?;
    write_u64(output, size)?;
    write_u64(output, size)
}

fn write_central_header<W: Write>(output: &mut W, entry: &WrittenEntry) -> Result<()> {
    let name_length = name_length(&entry.name)?;
    write_u32(output, CENTRAL_HEADER_SIGNATURE)?;
    write_u16(output, VERSION_MADE_BY)?;
    write_u16(output, ZIP64_VERSION)?;
    write_u16(output, 0)?;
    write_u16(output, 0)?;
    write_u16(output, DOS_TIME)?;
    write_u16(output, DOS_DATE)?;
    write_u32(output, entry.crc32)?;
    write_u32(output, u32::MAX)?;
    write_u32(output, u32::MAX)?;
    write_u16(output, name_length)?;
    write_u16(output, CENTRAL_ZIP64_EXTRA_BYTES)?;
    write_u16(output, 0)?;
    write_u16(output, 0)?;
    write_u16(output, 0)?;
    write_u32(output, 0)?;
    write_u32(output, u32::MAX)?;
    write_all(output, entry.name.as_bytes())?;
    write_u16(output, ZIP64_EXTRA_ID)?;
    write_u16(output, 24)?;
    write_u64(output, entry.size)?;
    write_u64(output, entry.size)?;
    write_u64(output, entry.local_offset)
}

fn copy_entry<W: Write>(output: &mut W, entry: &mut EntryWrite<'_>) -> Result<()> {
    let mut remaining = entry.size;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut hasher = Hasher::new();
    while remaining > 0 {
        let requested = usize::try_from(remaining.min(COPY_BUFFER_BYTES_U64)).map_err(|_| {
            CartridgeError::new(
                ErrorCode::EntrySizeMismatch,
                "copy size does not fit memory",
            )
            .at_entry(entry.name)
        })?;
        let read = entry
            .reader
            .read(&mut buffer[..requested])
            .map_err(|error| read_error("cannot read source entry", error))?;
        if read == 0 {
            return Err(CartridgeError::new(
                ErrorCode::EntrySizeMismatch,
                "source ended before its declared size",
            )
            .at_entry(entry.name));
        }
        write_all(output, &buffer[..read])?;
        hasher.update(&buffer[..read]);
        let read_length = u64::try_from(read).map_err(|error| {
            CartridgeError::new(ErrorCode::EntrySizeMismatch, "read length does not fit u64")
                .at_entry(entry.name)
                .with_source(error)
        })?;
        remaining = remaining.checked_sub(read_length).ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::EntrySizeMismatch,
                "source read exceeds declared size",
            )
            .at_entry(entry.name)
        })?;
    }

    let mut extra = [0_u8; 1];
    if entry
        .reader
        .read(&mut extra)
        .map_err(|error| read_error("cannot verify source length", error))?
        != 0
    {
        return Err(CartridgeError::new(
            ErrorCode::EntrySizeMismatch,
            "source contains bytes beyond its declared size",
        )
        .at_entry(entry.name));
    }
    if hasher.finalize() != entry.crc32 {
        return Err(
            CartridgeError::new(ErrorCode::EntryCrcMismatch, "source CRC-32 changed")
                .at_entry(entry.name),
        );
    }
    Ok(())
}

fn write_zip64_terminator<W: Write>(
    output: &mut W,
    entry_count: u64,
    central_size: u64,
    central_offset: u64,
) -> Result<()> {
    let zip64_end_offset = central_offset.checked_add(central_size).ok_or_else(|| {
        CartridgeError::new(ErrorCode::ArchiveMalformed, "archive offset overflow")
    })?;
    write_u32(output, ZIP64_END_SIGNATURE)?;
    write_u64(output, 44)?;
    write_u16(output, VERSION_MADE_BY)?;
    write_u16(output, ZIP64_VERSION)?;
    write_u32(output, 0)?;
    write_u32(output, 0)?;
    write_u64(output, entry_count)?;
    write_u64(output, entry_count)?;
    write_u64(output, central_size)?;
    write_u64(output, central_offset)?;

    write_u32(output, ZIP64_LOCATOR_SIGNATURE)?;
    write_u32(output, 0)?;
    write_u64(output, zip64_end_offset)?;
    write_u32(output, 1)?;

    write_u32(output, END_SIGNATURE)?;
    write_u16(output, 0)?;
    write_u16(output, 0)?;
    write_u16(output, u16::MAX)?;
    write_u16(output, u16::MAX)?;
    write_u32(output, u32::MAX)?;
    write_u32(output, u32::MAX)?;
    write_u16(output, 0)
}

/// Inspects the full archive structure without allocating tensor payloads.
///
/// # Errors
///
/// Returns an error when the archive is too large, cannot be read, or differs
/// from the strict LC 0.1 ZIP64 representation.
pub fn inspect_canonical<R>(input: &mut R, maximum_archive_bytes: u64) -> Result<ArchiveIndex>
where
    R: Read + Seek,
{
    let archive_size = input
        .seek(SeekFrom::End(0))
        .map_err(|error| read_error("cannot determine archive size", error))?;
    if archive_size > maximum_archive_bytes {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveTooLarge,
            "archive exceeds the configured byte limit",
        ));
    }
    if archive_size < ZIP64_END_BYTES + ZIP64_LOCATOR_BYTES + END_BYTES {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveMalformed,
            "archive is shorter than its required ZIP64 terminator",
        ));
    }

    let end_offset = find_end_record(input, archive_size)?;
    let terminator = read_terminator(input, archive_size, end_offset)?;
    let central_entries = read_central_entries(
        input,
        terminator.central_offset,
        terminator.central_size,
        terminator.entry_count,
    )?;
    let entries = read_local_entries(input, &central_entries, terminator.central_offset)?;
    Ok(ArchiveIndex {
        entries,
        archive_size,
    })
}

/// Streams one indexed entry and verifies the CRC-32 recorded by the archive.
///
/// # Errors
///
/// Returns an error when the indexed range cannot be read in full or its
/// checksum differs from the validated archive record.
pub fn verify_entry<R>(input: &mut R, entry: &ArchiveEntry) -> Result<()>
where
    R: Read + Seek,
{
    input
        .seek(SeekFrom::Start(entry.data_offset))
        .map_err(|error| read_error("cannot seek to entry payload", error))?;
    let mut remaining = entry.size;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut hasher = Hasher::new();
    while remaining > 0 {
        let requested = usize::try_from(remaining.min(COPY_BUFFER_BYTES_U64)).map_err(|_| {
            CartridgeError::new(
                ErrorCode::EntrySizeMismatch,
                "copy size does not fit memory",
            )
            .at_entry(&entry.name)
        })?;
        let read = input
            .read(&mut buffer[..requested])
            .map_err(|error| read_error("cannot read indexed entry", error))?;
        if read == 0 {
            return Err(CartridgeError::new(
                ErrorCode::EntrySizeMismatch,
                "indexed entry ended before its declared size",
            )
            .at_entry(&entry.name));
        }
        hasher.update(&buffer[..read]);
        let read_length = u64::try_from(read).map_err(|error| {
            CartridgeError::new(ErrorCode::EntrySizeMismatch, "read length does not fit u64")
                .at_entry(&entry.name)
                .with_source(error)
        })?;
        remaining = remaining.checked_sub(read_length).ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::EntrySizeMismatch,
                "entry read exceeds declared size",
            )
            .at_entry(&entry.name)
        })?;
    }
    if hasher.finalize() != entry.crc32 {
        return Err(CartridgeError::new(
            ErrorCode::EntryCrcMismatch,
            "entry CRC-32 does not match",
        )
        .at_entry(&entry.name));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct Terminator {
    entry_count: u64,
    central_size: u64,
    central_offset: u64,
}

fn find_end_record<R: Read + Seek>(input: &mut R, archive_size: u64) -> Result<u64> {
    let search_length = archive_size.min(u64::from(u16::MAX) + END_BYTES);
    let search_start = archive_size.checked_sub(search_length).ok_or_else(|| {
        CartridgeError::new(ErrorCode::ArchiveMalformed, "end search offset underflow")
    })?;
    input
        .seek(SeekFrom::Start(search_start))
        .map_err(|error| read_error("cannot seek to archive terminator", error))?;
    let allocation = usize::try_from(search_length).map_err(|_| {
        CartridgeError::new(ErrorCode::ArchiveTooLarge, "terminator search is too large")
    })?;
    let mut tail = vec![0_u8; allocation];
    input
        .read_exact(&mut tail)
        .map_err(|error| read_error("cannot read archive terminator", error))?;
    let signature = END_SIGNATURE.to_le_bytes();
    let relative = tail
        .windows(signature.len())
        .rposition(|window| window == signature)
        .ok_or_else(|| {
            CartridgeError::new(ErrorCode::ArchiveMalformed, "ZIP end record is missing")
        })?;
    let offset = search_start
        .checked_add(u64::try_from(relative).expect("tail offset fits u64"))
        .ok_or_else(|| {
            CartridgeError::new(ErrorCode::ArchiveMalformed, "end record offset overflow")
        })?;
    if offset.checked_add(END_BYTES) != Some(archive_size) {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveTrailingData,
            "bytes follow the canonical ZIP end record",
        ));
    }
    Ok(offset)
}

#[allow(clippy::too_many_lines)]
fn read_terminator<R: Read + Seek>(
    input: &mut R,
    archive_size: u64,
    end_offset: u64,
) -> Result<Terminator> {
    input
        .seek(SeekFrom::Start(end_offset))
        .map_err(|error| read_error("cannot seek to ZIP end record", error))?;
    expect_u32(
        input,
        END_SIGNATURE,
        ErrorCode::ArchiveMalformed,
        "ZIP end signature",
    )?;
    let disk = read_u16(input)?;
    let central_disk = read_u16(input)?;
    let entries_on_disk = read_u16(input)?;
    let total_entries = read_u16(input)?;
    let central_size_sentinel = read_u32(input)?;
    let central_offset_sentinel = read_u32(input)?;
    let comment_length = read_u16(input)?;
    if disk != 0
        || central_disk != 0
        || entries_on_disk != u16::MAX
        || total_entries != u16::MAX
        || central_size_sentinel != u32::MAX
        || central_offset_sentinel != u32::MAX
        || comment_length != 0
    {
        return Err(CartridgeError::new(
            ErrorCode::Zip64Required,
            "classic end record must contain the canonical ZIP64 sentinels",
        ));
    }

    let locator_offset = end_offset.checked_sub(ZIP64_LOCATOR_BYTES).ok_or_else(|| {
        CartridgeError::new(
            ErrorCode::ArchiveMalformed,
            "ZIP64 locator offset underflow",
        )
    })?;
    input
        .seek(SeekFrom::Start(locator_offset))
        .map_err(|error| read_error("cannot seek to ZIP64 locator", error))?;
    expect_u32(
        input,
        ZIP64_LOCATOR_SIGNATURE,
        ErrorCode::Zip64Required,
        "ZIP64 locator signature",
    )?;
    let end_disk = read_u32(input)?;
    let zip64_end_offset = read_u64(input)?;
    let disk_count = read_u32(input)?;
    if end_disk != 0 || disk_count != 1 {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveNoncanonical,
            "multi-disk ZIP archives are forbidden",
        ));
    }
    if zip64_end_offset.checked_add(ZIP64_END_BYTES) != Some(locator_offset) {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveNoncanonical,
            "ZIP64 records are not contiguous",
        ));
    }

    input
        .seek(SeekFrom::Start(zip64_end_offset))
        .map_err(|error| read_error("cannot seek to ZIP64 end record", error))?;
    expect_u32(
        input,
        ZIP64_END_SIGNATURE,
        ErrorCode::Zip64Required,
        "ZIP64 end signature",
    )?;
    let record_size = read_u64(input)?;
    let made_by = read_u16(input)?;
    let needed = read_u16(input)?;
    let disk = read_u32(input)?;
    let central_disk = read_u32(input)?;
    let entries_on_disk = read_u64(input)?;
    let entry_count = read_u64(input)?;
    let central_size = read_u64(input)?;
    let central_offset = read_u64(input)?;
    if record_size != 44
        || made_by != VERSION_MADE_BY
        || needed != ZIP64_VERSION
        || disk != 0
        || central_disk != 0
        || entries_on_disk != entry_count
    {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveNoncanonical,
            "ZIP64 end record does not match the LC canonical form",
        ));
    }
    let count = usize::try_from(entry_count).map_err(|_| {
        CartridgeError::new(
            ErrorCode::EntryCountInvalid,
            "entry count does not fit memory",
        )
    })?;
    if !(MIN_ARCHIVE_ENTRIES..=MAX_ARCHIVE_ENTRIES).contains(&count) {
        return Err(CartridgeError::new(
            ErrorCode::EntryCountInvalid,
            "LC 0.1 requires two entries and permits one optional preview",
        ));
    }
    if central_offset.checked_add(central_size) != Some(zip64_end_offset)
        || end_offset.checked_add(END_BYTES) != Some(archive_size)
    {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveNoncanonical,
            "central directory or terminator offsets are inconsistent",
        ));
    }
    Ok(Terminator {
        entry_count,
        central_size,
        central_offset,
    })
}

fn read_central_entries<R: Read + Seek>(
    input: &mut R,
    central_offset: u64,
    central_size: u64,
    entry_count: u64,
) -> Result<Vec<CentralEntry>> {
    input
        .seek(SeekFrom::Start(central_offset))
        .map_err(|error| read_error("cannot seek to central directory", error))?;
    let capacity = usize::try_from(entry_count).map_err(|_| {
        CartridgeError::new(
            ErrorCode::EntryCountInvalid,
            "entry count does not fit memory",
        )
    })?;
    let mut entries = Vec::with_capacity(capacity);
    let mut folded_names = HashSet::with_capacity(capacity);
    for _ in 0..entry_count {
        let entry = read_central_entry(input)?;
        validate_safe_name(&entry.name)?;
        validate_entry_size(&entry.name, entry.size)?;
        if !folded_names.insert(entry.name.to_ascii_lowercase()) {
            return Err(
                CartridgeError::new(ErrorCode::EntryDuplicate, "entry names collide")
                    .at_entry(&entry.name),
            );
        }
        entries.push(entry);
    }
    let central_end = input
        .stream_position()
        .map_err(|error| read_error("cannot query central directory end", error))?;
    if central_offset.checked_add(central_size) != Some(central_end) {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveNoncanonical,
            "central directory size does not match its records",
        ));
    }
    validate_entry_order(entries.iter().map(|entry| entry.name.as_str()))?;
    Ok(entries)
}

fn read_central_entry<R: Read>(input: &mut R) -> Result<CentralEntry> {
    expect_u32(
        input,
        CENTRAL_HEADER_SIGNATURE,
        ErrorCode::ArchiveMalformed,
        "central header signature",
    )?;
    let made_by = read_u16(input)?;
    let needed = read_u16(input)?;
    let flags = read_u16(input)?;
    let method = read_u16(input)?;
    let modified_time = read_u16(input)?;
    let modified_date = read_u16(input)?;
    let crc32 = read_u32(input)?;
    let compressed_size = read_u32(input)?;
    let uncompressed_size = read_u32(input)?;
    let name_length = read_u16(input)?;
    let extra_length = read_u16(input)?;
    let comment_length = read_u16(input)?;
    let disk_start = read_u16(input)?;
    let internal_attributes = read_u16(input)?;
    let external_attributes = read_u32(input)?;
    let local_offset_sentinel = read_u32(input)?;

    if made_by != VERSION_MADE_BY {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveNoncanonical,
            "central entry host and writer version are not canonical",
        ));
    }
    validate_common_header(needed, flags, method, modified_time, modified_date)?;
    if compressed_size != u32::MAX
        || uncompressed_size != u32::MAX
        || extra_length != CENTRAL_ZIP64_EXTRA_BYTES
        || comment_length != 0
        || disk_start != 0
        || internal_attributes != 0
        || external_attributes != 0
        || local_offset_sentinel != u32::MAX
    {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveNoncanonical,
            "central entry is not in the canonical ZIP64 form",
        ));
    }
    let name = read_name(input, name_length)?;
    let extra_id = read_u16(input)?;
    let extra_payload_length = read_u16(input)?;
    let size = read_u64(input)?;
    let compressed_size_64 = read_u64(input)?;
    let local_offset = read_u64(input)?;
    if extra_id != ZIP64_EXTRA_ID || extra_payload_length != 24 || size != compressed_size_64 {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveNoncanonical,
            "central ZIP64 extra field is invalid",
        )
        .at_entry(&name));
    }
    Ok(CentralEntry {
        name,
        size,
        crc32,
        local_offset,
    })
}

fn read_local_entries<R: Read + Seek>(
    input: &mut R,
    central_entries: &[CentralEntry],
    central_offset: u64,
) -> Result<Vec<ArchiveEntry>> {
    let mut expected_offset = 0_u64;
    let mut entries = Vec::with_capacity(central_entries.len());
    for central in central_entries {
        if central.local_offset != expected_offset {
            return Err(CartridgeError::new(
                ErrorCode::EntryOverlap,
                "local entries are not contiguous and ordered",
            )
            .at_entry(&central.name));
        }
        input
            .seek(SeekFrom::Start(central.local_offset))
            .map_err(|error| read_error("cannot seek to local entry", error))?;
        expect_u32(
            input,
            LOCAL_HEADER_SIGNATURE,
            ErrorCode::ArchiveMalformed,
            "local header signature",
        )?;
        let needed = read_u16(input)?;
        let flags = read_u16(input)?;
        let method = read_u16(input)?;
        let modified_time = read_u16(input)?;
        let modified_date = read_u16(input)?;
        let crc32 = read_u32(input)?;
        let compressed_size = read_u32(input)?;
        let uncompressed_size = read_u32(input)?;
        let name_length = read_u16(input)?;
        let extra_length = read_u16(input)?;
        validate_common_header(needed, flags, method, modified_time, modified_date)?;
        if compressed_size != u32::MAX
            || uncompressed_size != u32::MAX
            || extra_length != LOCAL_ZIP64_EXTRA_BYTES
        {
            return Err(CartridgeError::new(
                ErrorCode::ArchiveNoncanonical,
                "local entry is not in the canonical ZIP64 form",
            )
            .at_entry(&central.name));
        }
        let name = read_name(input, name_length)?;
        let extra_id = read_u16(input)?;
        let extra_payload_length = read_u16(input)?;
        let size = read_u64(input)?;
        let compressed_size_64 = read_u64(input)?;
        if extra_id != ZIP64_EXTRA_ID || extra_payload_length != 16 || size != compressed_size_64 {
            return Err(CartridgeError::new(
                ErrorCode::ArchiveNoncanonical,
                "local ZIP64 extra field is invalid",
            )
            .at_entry(&name));
        }
        if name != central.name || size != central.size || crc32 != central.crc32 {
            return Err(CartridgeError::new(
                ErrorCode::EntrySizeMismatch,
                "local and central entry records differ",
            )
            .at_entry(&central.name));
        }
        let data_offset = input
            .stream_position()
            .map_err(|error| read_error("cannot query entry data offset", error))?;
        let data_end = data_offset.checked_add(size).ok_or_else(|| {
            CartridgeError::new(ErrorCode::EntrySizeMismatch, "entry range overflows u64")
                .at_entry(&central.name)
        })?;
        if data_end > central_offset {
            return Err(CartridgeError::new(
                ErrorCode::EntryOverlap,
                "entry payload overlaps the central directory",
            )
            .at_entry(&central.name));
        }
        expected_offset = data_end;
        entries.push(ArchiveEntry {
            name,
            size,
            crc32,
            data_offset,
        });
    }
    if expected_offset != central_offset {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveNoncanonical,
            "bytes exist between entry data and the central directory",
        ));
    }
    Ok(entries)
}

fn validate_common_header(
    needed: u16,
    flags: u16,
    method: u16,
    modified_time: u16,
    modified_date: u16,
) -> Result<()> {
    if flags & 0x0001 != 0 || flags & 0x0040 != 0 {
        return Err(CartridgeError::new(
            ErrorCode::EntryEncrypted,
            "encrypted ZIP entries are forbidden",
        ));
    }
    if flags != 0 {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveNoncanonical,
            "ZIP entry flags must be zero",
        ));
    }
    if method != 0 {
        return Err(CartridgeError::new(
            ErrorCode::EntryCompressed,
            "LC entries must use the STORE method",
        ));
    }
    if needed != ZIP64_VERSION || modified_time != DOS_TIME || modified_date != DOS_DATE {
        return Err(CartridgeError::new(
            ErrorCode::ArchiveNoncanonical,
            "ZIP version or timestamp is not canonical",
        ));
    }
    Ok(())
}

fn read_name<R: Read>(input: &mut R, length: u16) -> Result<String> {
    let mut bytes = vec![0_u8; usize::from(length)];
    input
        .read_exact(&mut bytes)
        .map_err(|error| read_error("cannot read entry name", error))?;
    String::from_utf8(bytes)
        .map_err(|error| CartridgeError::new(ErrorCode::EntryUnsafePath, error.to_string()))
}

fn name_length(name: &str) -> Result<u16> {
    u16::try_from(name.len()).map_err(|_| {
        CartridgeError::new(
            ErrorCode::EntryUnsafePath,
            "entry name exceeds the ZIP limit",
        )
        .at_entry(name)
    })
}

fn expect_u32<R: Read>(input: &mut R, expected: u32, code: ErrorCode, label: &str) -> Result<()> {
    let actual = read_u32(input)?;
    if actual == expected {
        Ok(())
    } else {
        Err(CartridgeError::new(code, format!("{label} is invalid")))
    }
}

fn read_u16<R: Read>(input: &mut R) -> Result<u16> {
    let mut bytes = [0_u8; 2];
    input
        .read_exact(&mut bytes)
        .map_err(|error| read_error("cannot read u16", error))?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32<R: Read>(input: &mut R) -> Result<u32> {
    let mut bytes = [0_u8; 4];
    input
        .read_exact(&mut bytes)
        .map_err(|error| read_error("cannot read u32", error))?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64<R: Read>(input: &mut R) -> Result<u64> {
    let mut bytes = [0_u8; 8];
    input
        .read_exact(&mut bytes)
        .map_err(|error| read_error("cannot read u64", error))?;
    Ok(u64::from_le_bytes(bytes))
}

fn write_u16<W: Write>(output: &mut W, value: u16) -> Result<()> {
    write_all(output, &value.to_le_bytes())
}

fn write_u32<W: Write>(output: &mut W, value: u32) -> Result<()> {
    write_all(output, &value.to_le_bytes())
}

fn write_u64<W: Write>(output: &mut W, value: u64) -> Result<()> {
    write_all(output, &value.to_le_bytes())
}

fn write_all<W: Write>(output: &mut W, bytes: &[u8]) -> Result<()> {
    output
        .write_all(bytes)
        .map_err(|error| write_error("cannot write canonical archive", error))
}

fn read_error(detail: &str, source: std::io::Error) -> CartridgeError {
    CartridgeError::new(ErrorCode::IoRead, detail).with_source(source)
}

fn write_error(detail: &str, source: std::io::Error) -> CartridgeError {
    CartridgeError::new(ErrorCode::IoWrite, detail).with_source(source)
}
