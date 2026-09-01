//! Narrow Windows Media Foundation boundary.
//!
//! Every unsafe call is contained here. Inputs have already passed the safe
//! bounded frame contract; COM objects remain confined to the writer thread.

#![allow(unsafe_code)]

use std::{ffi::OsStr, os::windows::ffi::OsStrExt as _, path::Path, ptr};

use windows::{
    Win32::{
        Media::MediaFoundation::{
            IMFMediaBuffer, IMFSinkWriter, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE,
            MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
            MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_VERSION, MFCreateMediaType,
            MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Video,
            MFSTARTUP_FULL, MFShutdown, MFStartup, MFVideoFormat_H264, MFVideoFormat_RGB32,
            MFVideoInterlace_Progressive,
        },
        Storage::FileSystem::MoveFileW,
        System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize},
    },
    core::PCWSTR,
};

use super::{RecorderError, VIDEO_FPS_DENOMINATOR, VIDEO_FPS_NUMERATOR};

const HUNDRED_NS_PER_SECOND: u64 = 10_000_000;

pub(super) fn publish_no_clobber(
    temporary: &Path,
    destination: &Path,
) -> Result<(), RecorderError> {
    let temporary = wide_path(temporary);
    let destination_wide = wide_path(destination);
    // SAFETY: both NUL-terminated buffers remain live for the synchronous call.
    // MoveFileW has no replace-existing flag and therefore provides the
    // publication primitive required by the no-clobber contract.
    unsafe {
        MoveFileW(
            PCWSTR(temporary.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
        )
    }
    .map_err(|_| {
        if destination.exists() {
            RecorderError::OutputExists
        } else {
            RecorderError::FinalizeFailed
        }
    })
}

fn wide_path(path: &Path) -> Vec<u16> {
    OsStr::new(path.as_os_str())
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub(super) struct VideoSink {
    writer: Option<IMFSinkWriter>,
    stream: u32,
    width: u32,
    height: u32,
    frame_index: u64,
    lifecycle: Option<MediaFoundationApartment>,
}

struct MediaFoundationApartment {
    media_foundation_started: bool,
    com_initialized: bool,
}

impl MediaFoundationApartment {
    fn start() -> Result<Self, RecorderError> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|_| RecorderError::EncoderUnavailable)?;
            if MFStartup(MF_VERSION, MFSTARTUP_FULL).is_err() {
                CoUninitialize();
                return Err(RecorderError::EncoderUnavailable);
            }
        }
        Ok(Self {
            media_foundation_started: true,
            com_initialized: true,
        })
    }

    fn shutdown(mut self) -> Result<(), RecorderError> {
        let media_result = if self.media_foundation_started {
            self.media_foundation_started = false;
            unsafe { MFShutdown().map_err(|_| RecorderError::FinalizeFailed) }
        } else {
            Ok(())
        };
        if self.com_initialized {
            self.com_initialized = false;
            unsafe { CoUninitialize() };
        }
        media_result
    }
}

impl Drop for MediaFoundationApartment {
    fn drop(&mut self) {
        if self.media_foundation_started {
            unsafe {
                let _ = MFShutdown();
            }
            self.media_foundation_started = false;
        }
        if self.com_initialized {
            unsafe { CoUninitialize() };
            self.com_initialized = false;
        }
    }
}

impl VideoSink {
    pub(super) fn open(path: &Path, width: u32, height: u32) -> Result<Self, RecorderError> {
        let wide = wide_path(path);
        // SAFETY: this fresh dedicated thread owns its COM apartment until
        // Drop; pointers passed below stay live for each synchronous call.
        let lifecycle = MediaFoundationApartment::start()?;
        unsafe {
            let writer = MFCreateSinkWriterFromURL(
                PCWSTR(wide.as_ptr()),
                None::<&windows::Win32::Media::MediaFoundation::IMFByteStream>,
                None::<&windows::Win32::Media::MediaFoundation::IMFAttributes>,
            )
            .map_err(|_| RecorderError::EncoderUnavailable)?;

            let output_type = MFCreateMediaType().map_err(|_| RecorderError::EncoderUnavailable)?;
            output_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .and_then(|()| output_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264))
                .and_then(|()| output_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate(width, height)))
                .and_then(|()| {
                    output_type.SetUINT32(
                        &MF_MT_INTERLACE_MODE,
                        MFVideoInterlace_Progressive.0.cast_unsigned(),
                    )
                })
                .and_then(|()| output_type.SetUINT64(&MF_MT_FRAME_SIZE, pair(width, height)))
                .and_then(|()| {
                    output_type.SetUINT64(
                        &MF_MT_FRAME_RATE,
                        pair(VIDEO_FPS_NUMERATOR, VIDEO_FPS_DENOMINATOR),
                    )
                })
                .and_then(|()| output_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pair(1, 1)))
                .map_err(|_| RecorderError::EncoderUnavailable)?;
            let stream = writer
                .AddStream(&output_type)
                .map_err(|_| RecorderError::EncoderUnavailable)?;

            let input_type = MFCreateMediaType().map_err(|_| RecorderError::EncoderUnavailable)?;
            input_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .and_then(|()| input_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32))
                .and_then(|()| {
                    input_type.SetUINT32(
                        &MF_MT_INTERLACE_MODE,
                        MFVideoInterlace_Progressive.0.cast_unsigned(),
                    )
                })
                .and_then(|()| input_type.SetUINT64(&MF_MT_FRAME_SIZE, pair(width, height)))
                .and_then(|()| {
                    input_type.SetUINT64(
                        &MF_MT_FRAME_RATE,
                        pair(VIDEO_FPS_NUMERATOR, VIDEO_FPS_DENOMINATOR),
                    )
                })
                .and_then(|()| input_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pair(1, 1)))
                .and_then(|()| input_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4))
                .and_then(|()| {
                    writer.SetInputMediaType(
                        stream,
                        &input_type,
                        None::<&windows::Win32::Media::MediaFoundation::IMFAttributes>,
                    )
                })
                .and_then(|()| writer.BeginWriting())
                .map_err(|_| RecorderError::EncoderUnavailable)?;

            Ok(Self {
                writer: Some(writer),
                stream,
                width,
                height,
                frame_index: 0,
                lifecycle: Some(lifecycle),
            })
        }
    }

    pub(super) fn write_rgba(
        &mut self,
        width: u32,
        height: u32,
        row_stride: u32,
        rgba: &[u8],
    ) -> Result<(), RecorderError> {
        if (width, height) != (self.width, self.height) {
            return Err(RecorderError::InvalidFrame);
        }
        let packed_len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(RecorderError::InvalidFrame)?;
        let buffer =
            unsafe { MFCreateMemoryBuffer(packed_len) }.map_err(|_| RecorderError::EncodeFailed)?;
        fill_rgb32_top_down(&buffer, width, height, row_stride, rgba)?;
        let sample = unsafe { MFCreateSample() }.map_err(|_| RecorderError::EncodeFailed)?;
        let (start, duration) = frame_time(self.frame_index)?;
        let writer = self.writer.as_ref().ok_or(RecorderError::EncodeFailed)?;
        unsafe {
            sample
                .AddBuffer(&buffer)
                .and_then(|()| sample.SetSampleTime(start))
                .and_then(|()| sample.SetSampleDuration(duration))
                .and_then(|()| writer.WriteSample(self.stream, &sample))
                .map_err(|_| RecorderError::EncodeFailed)?;
        }
        self.frame_index = self
            .frame_index
            .checked_add(1)
            .ok_or(RecorderError::EncodeFailed)?;
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<(), RecorderError> {
        let writer = self.writer.take().ok_or(RecorderError::FinalizeFailed)?;
        unsafe {
            writer
                .Finalize()
                .map_err(|_| RecorderError::FinalizeFailed)?;
        }
        drop(writer);
        self.lifecycle
            .take()
            .ok_or(RecorderError::FinalizeFailed)?
            .shutdown()
    }
}

impl Drop for VideoSink {
    fn drop(&mut self) {
        drop(self.writer.take());
        drop(self.lifecycle.take());
    }
}

fn fill_rgb32_top_down(
    buffer: &IMFMediaBuffer,
    width: u32,
    height: u32,
    source_stride: u32,
    rgba: &[u8],
) -> Result<(), RecorderError> {
    let packed_stride = usize::try_from(width * 4).map_err(|_| RecorderError::InvalidFrame)?;
    let source_stride = usize::try_from(source_stride).map_err(|_| RecorderError::InvalidFrame)?;
    let height = usize::try_from(height).map_err(|_| RecorderError::InvalidFrame)?;
    let mut destination = ptr::null_mut();
    let mut maximum = 0_u32;
    unsafe {
        buffer
            .Lock(&raw mut destination, Some(&raw mut maximum), None)
            .map_err(|_| RecorderError::EncodeFailed)?;
    }
    let packed_length = packed_stride
        .checked_mul(height)
        .ok_or(RecorderError::InvalidFrame)?;
    if destination.is_null() || usize::try_from(maximum).unwrap_or(0) < packed_length {
        unsafe {
            let _ = buffer.Unlock();
        }
        return Err(RecorderError::EncodeFailed);
    }
    for output_y in 0..height {
        let source_y = output_y;
        let source_row = &rgba[source_y * source_stride..source_y * source_stride + packed_stride];
        let destination_row = unsafe { destination.add(output_y * packed_stride) };
        for (x, pixel) in source_row.chunks_exact(4).enumerate() {
            let target = unsafe { destination_row.add(x * 4) };
            unsafe {
                *target = pixel[2];
                *target.add(1) = pixel[1];
                *target.add(2) = pixel[0];
                *target.add(3) = 0;
            }
        }
    }
    unsafe {
        buffer
            .Unlock()
            .and_then(|()| buffer.SetCurrentLength(u32::try_from(packed_length).unwrap_or(0)))
            .map_err(|_| RecorderError::EncodeFailed)
    }
}

fn pair(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

fn bitrate(width: u32, height: u32) -> u32 {
    width
        .saturating_mul(height)
        .saturating_mul(VIDEO_FPS_NUMERATOR)
        .saturating_div(4)
        .clamp(2_000_000, 30_000_000)
}

fn frame_time(index: u64) -> Result<(i64, i64), RecorderError> {
    let ticks = |frame: u64| {
        u128::from(frame)
            .checked_mul(u128::from(HUNDRED_NS_PER_SECOND))?
            .checked_mul(u128::from(VIDEO_FPS_DENOMINATOR))?
            .checked_div(u128::from(VIDEO_FPS_NUMERATOR))
    };
    let start = ticks(index).ok_or(RecorderError::EncodeFailed)?;
    let end = ticks(index.checked_add(1).ok_or(RecorderError::EncodeFailed)?)
        .ok_or(RecorderError::EncodeFailed)?;
    Ok((
        i64::try_from(start).map_err(|_| RecorderError::EncodeFailed)?,
        i64::try_from(end - start).map_err(|_| RecorderError::EncodeFailed)?,
    ))
}

#[cfg(test)]
mod tests {
    use std::slice;

    use super::*;

    #[test]
    fn rgb32_packing_preserves_top_down_row_order() {
        let _lifecycle = MediaFoundationApartment::start().expect("Media Foundation starts");
        let buffer = unsafe { MFCreateMemoryBuffer(16) }.expect("RGB32 test buffer");
        let rgba = [
            1, 2, 3, 255, 4, 5, 6, 255, 90, 91, 92, 93, // top row plus padding
            7, 8, 9, 255, 10, 11, 12, 255, 94, 95, 96, 97, // bottom row plus padding
        ];

        fill_rgb32_top_down(&buffer, 2, 2, 12, &rgba).expect("pack top-down RGB32");

        let mut data = ptr::null_mut();
        let mut maximum = 0_u32;
        let mut current = 0_u32;
        unsafe {
            buffer
                .Lock(
                    &raw mut data,
                    Some(&raw mut maximum),
                    Some(&raw mut current),
                )
                .expect("lock packed RGB32");
        }
        let packed = unsafe { slice::from_raw_parts(data, current as usize) }.to_vec();
        unsafe {
            buffer.Unlock().expect("unlock packed RGB32");
        }

        assert_eq!(maximum, 16);
        assert_eq!(current, 16);
        assert_eq!(packed, [3, 2, 1, 0, 6, 5, 4, 0, 9, 8, 7, 0, 12, 11, 10, 0]);
    }
}
