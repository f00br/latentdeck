//! Narrow native boundary for the original `LatentDeck` C ABI bridge.
//!
//! This is the only Rust module allowed to call the native ABI. Every incoming
//! COM interface is borrowed from a safe `windows` wrapper; the C++ bridge takes
//! its own COM references before the call returns. Return codes and the bounded
//! status buffer are validated before they become safe Rust values.

#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::NonNull;

use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Device};
use windows::core::Interface;

use crate::{Backend, BackendFault, BackendStatus, Dx12ResourceState, SenderConfig, SpoutError};

const STATUS_SCHEMA: u32 = 1;

#[repr(C)]
struct NativeSender {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeStatus {
    schema: u32,
    published: u32,
    width: u32,
    height: u32,
    format: u32,
    spout_frame: i64,
    active_name: [c_char; 256],
}

impl Default for NativeStatus {
    fn default() -> Self {
        Self {
            schema: 0,
            published: 0,
            width: 0,
            height: 0,
            format: 0,
            spout_frame: 0,
            active_name: [0; 256],
        }
    }
}

unsafe extern "C" {
    fn latentdeck_spout_sender_open(
        d3d12_device: *mut c_void,
        direct_command_queue: *mut c_void,
        sender_name: *const c_char,
        width: u32,
        height: u32,
        dxgi_format: u32,
        out_sender: *mut *mut NativeSender,
        out_status: *mut NativeStatus,
    ) -> i32;
    fn latentdeck_spout_sender_set_name(
        sender: *mut NativeSender,
        sender_name: *const c_char,
        out_status: *mut NativeStatus,
    ) -> i32;
    fn latentdeck_spout_sender_release(
        sender: *mut NativeSender,
        out_status: *mut NativeStatus,
    ) -> i32;
    fn latentdeck_spout_sender_send_resource(
        sender: *mut NativeSender,
        d3d12_resource: *mut c_void,
        initial_resource_state: u32,
        out_status: *mut NativeStatus,
    ) -> i32;
    fn latentdeck_spout_sender_destroy(sender: *mut NativeSender);
}

pub(crate) struct NativeBackend {
    sender: Option<NonNull<NativeSender>>,
}

// SAFETY: the C++ bridge owns every COM reference, enables
// `ID3D11Multithread::SetMultithreadProtected(TRUE)`, and is accessed only
// through `&mut self`. Moving the sole owner between actor polls cannot create
// concurrent native calls; `SpoutSender` remains explicitly `!Sync`.
unsafe impl Send for NativeBackend {}

impl NativeBackend {
    pub(crate) fn open(
        config: &SenderConfig,
        device: &ID3D12Device,
        direct_queue: &ID3D12CommandQueue,
    ) -> Result<Self, SpoutError> {
        let name = c_name(config.name()).map_err(SpoutError::Backend)?;
        let mut sender = std::ptr::null_mut();
        let mut status = NativeStatus::default();
        // SAFETY: `windows` wrappers guarantee live interface pointers. The C
        // ABI copies the bounded name and AddRefs device/queue before return.
        let result = unsafe {
            latentdeck_spout_sender_open(
                device.as_raw(),
                direct_queue.as_raw(),
                name.as_ptr(),
                config.width(),
                config.height(),
                config.format() as u32,
                &raw mut sender,
                &raw mut status,
            )
        };
        if let Err(fault) = map_result(result) {
            if let Some(sender) = NonNull::new(sender) {
                // SAFETY: a non-null error handle can only be a bridge-owned
                // partially opened sender; destroying it closes retained COM refs.
                unsafe { latentdeck_spout_sender_destroy(sender.as_ptr()) };
            }
            return Err(SpoutError::Backend(fault));
        }
        let sender = NonNull::new(sender).ok_or(BackendFault::Internal)?;
        let backend = Self {
            sender: Some(sender),
        };
        decode_status(&status).map_err(SpoutError::Backend)?;
        Ok(backend)
    }

    fn sender(&self) -> Result<NonNull<NativeSender>, BackendFault> {
        self.sender.ok_or(BackendFault::Internal)
    }
}

impl Backend for NativeBackend {
    fn set_name(&mut self, name: &str) -> Result<BackendStatus, BackendFault> {
        let name = c_name(name)?;
        let mut status = NativeStatus::default();
        // SAFETY: the live handle is owned by `self`; the ABI only copies `name`.
        let result = unsafe {
            latentdeck_spout_sender_set_name(
                self.sender()?.as_ptr(),
                name.as_ptr(),
                &raw mut status,
            )
        };
        map_result(result)?;
        decode_status(&status)
    }

    fn release_sender(&mut self) -> Result<BackendStatus, BackendFault> {
        let mut status = NativeStatus::default();
        // SAFETY: the live handle is uniquely controlled by `self` on one thread.
        let result =
            unsafe { latentdeck_spout_sender_release(self.sender()?.as_ptr(), &raw mut status) };
        map_result(result)?;
        decode_status(&status)
    }

    fn send(
        &mut self,
        resource: *mut c_void,
        initial_state: Dx12ResourceState,
    ) -> Result<BackendStatus, BackendFault> {
        let mut status = NativeStatus::default();
        // SAFETY: public callers provide a live `ID3D12Resource` reference. The
        // bridge validates its device/description and AddRefs cached resources.
        let result = unsafe {
            latentdeck_spout_sender_send_resource(
                self.sender()?.as_ptr(),
                resource,
                initial_state as u32,
                &raw mut status,
            )
        };
        map_result(result)?;
        decode_status(&status)
    }

    fn close(&mut self) {
        if let Some(sender) = self.sender.take() {
            // SAFETY: `take` makes this the one and only destroy for the handle.
            unsafe { latentdeck_spout_sender_destroy(sender.as_ptr()) };
        }
    }
}

impl Drop for NativeBackend {
    fn drop(&mut self) {
        self.close();
    }
}

fn c_name(name: &str) -> Result<CString, BackendFault> {
    CString::new(name).map_err(|_| BackendFault::StatusMismatch)
}

fn map_result(result: i32) -> Result<(), BackendFault> {
    match result {
        0 => Ok(()),
        1 | 3 | 8 => Err(BackendFault::IncompatibleDx12),
        2 => Err(BackendFault::OpenFailed),
        4 => Err(BackendFault::ResourceMismatch),
        5 => Err(BackendFault::WrapFailed),
        6 => Err(BackendFault::SendFailed),
        7 => Err(BackendFault::ResourceLimit),
        _ => Err(BackendFault::Internal),
    }
}

fn decode_status(status: &NativeStatus) -> Result<BackendStatus, BackendFault> {
    if status.schema != STATUS_SCHEMA || status.published > 1 {
        return Err(BackendFault::StatusMismatch);
    }
    // SAFETY: the ABI zero-initializes the fixed array and uses `strncpy_s`, so
    // it is NUL-terminated within these 256 bytes.
    let name = unsafe { CStr::from_ptr(status.active_name.as_ptr()) }
        .to_str()
        .map_err(|_| BackendFault::StatusMismatch)?
        .to_owned();
    Ok(BackendStatus {
        active_name: name,
        published: status.published == 1,
        width: status.width,
        height: status.height,
        format: status.format,
        spout_frame: (status.published == 1).then_some(status.spout_frame),
    })
}
