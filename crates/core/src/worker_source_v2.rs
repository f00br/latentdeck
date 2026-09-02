//! Native retained-cartridge transfer into one authenticated Protocol 2 worker.

use latentdeck_cartridge::{error::CartridgeError, reader::IntegrityValidatedCartridge};
use latentdeck_control::v2::{Command, SourceOpen};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    worker_client_v2::{WorkerClientV2, WorkerClientV2Error},
    worker_supervisor::WorkerSupervisorError,
};

#[cfg(windows)]
#[path = "worker_source_v2/windows.rs"]
mod platform;

/// Failures before an exact duplicated handle can be sent to a P2 worker.
#[derive(Debug, Error)]
pub enum WorkerSourceV2Error {
    #[error("validated LC access receipt could not be encoded")]
    Receipt(#[source] CartridgeError),
    #[error("validated LC manifest cartridge identity is invalid")]
    CartridgeIdentity,
    #[error("validated LC access receipt was not UTF-8")]
    ReceiptEncoding,
    #[error("retained LC handle could not be duplicated into the worker")]
    Duplicate(#[source] std::io::Error),
    #[error(transparent)]
    Client(#[from] WorkerClientV2Error),
    #[error(transparent)]
    Supervisor(#[from] WorkerSupervisorError),
    #[error("retained LC handle transfer is supported only on Windows")]
    UnsupportedPlatform,
}

/// Duplicate the exact no-share-write/delete LC handle into the authenticated
/// worker and build the closed `source.open` command that consumes it.
///
/// No archive, tensor, or manifest bytes enter the control frame. The
/// canonical receipt contains only bounded ranges, identities, and hashes.
///
/// # Errors
///
/// Returns an error for receipt encoding, manifest identity, process-handle,
/// or OS handle-duplication failures. It never falls back to reopening a path.
#[cfg(windows)]
pub fn prepare_source_open(
    client: &WorkerClientV2,
    cartridge: &IntegrityValidatedCartridge,
    source_id: Uuid,
) -> Result<Command, WorkerSourceV2Error> {
    if source_id.is_nil() {
        return Err(WorkerSourceV2Error::CartridgeIdentity);
    }
    let cartridge_text = &cartridge.manifest().cartridge_id.0;
    let cartridge_id = Uuid::parse_str(cartridge_text)
        .ok()
        .filter(|value| !value.is_nil() && value.hyphenated().to_string() == *cartridge_text)
        .ok_or(WorkerSourceV2Error::CartridgeIdentity)?;
    let receipt_bytes = cartridge
        .access_receipt()
        .canonical_json()
        .map_err(WorkerSourceV2Error::Receipt)?;
    let integrity_access_receipt =
        String::from_utf8(receipt_bytes).map_err(|_| WorkerSourceV2Error::ReceiptEncoding)?;
    let duplicated = client.with_process_handle(|target_process| {
        cartridge.with_retained_file_handle(|source_file| {
            platform::duplicate_into_process(source_file, target_process)
        })
    })?;
    let retained_native_handle = duplicated.map_err(WorkerSourceV2Error::Duplicate)?;

    Ok(Command::SourceOpen(SourceOpen {
        source_id,
        cartridge_id,
        archive_sha256: cartridge.receipt().archive_sha256.to_string(),
        archive_bytes: cartridge.receipt().archive_bytes,
        retained_native_handle,
        integrity_access_receipt,
    }))
}

/// Non-Windows compile surface for the Windows retained-handle bridge.
///
/// # Errors
///
/// Always returns [`WorkerSourceV2Error::UnsupportedPlatform`].
#[cfg(not(windows))]
pub fn prepare_source_open(
    _client: &WorkerClientV2,
    _cartridge: &IntegrityValidatedCartridge,
    _source_id: Uuid,
) -> Result<Command, WorkerSourceV2Error> {
    Err(WorkerSourceV2Error::UnsupportedPlatform)
}
