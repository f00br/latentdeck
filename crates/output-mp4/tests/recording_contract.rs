use std::fs;

use latentdeck_output_mp4::{Mp4Recorder, RecorderConfig, RecorderState};
use tempfile::tempdir;

#[test]
fn destination_validation_is_no_clobber_and_mp4_only() {
    let root = tempdir().expect("temporary directory");
    let wrong_extension = root.path().join("capture.mov");
    let error = Mp4Recorder::start(RecorderConfig::new(wrong_extension))
        .expect_err("non-MP4 destinations must be rejected");
    assert_eq!(error.code(), "recording.destination_invalid");

    let existing = root.path().join("capture.mp4");
    fs::write(&existing, b"owner data").expect("existing owner file");
    let error = Mp4Recorder::start(RecorderConfig::new(existing.clone()))
        .expect_err("existing output must not be overwritten");
    assert_eq!(error.code(), "recording.output_exists");
    assert_eq!(
        fs::read(existing).expect("owner file remains"),
        b"owner data"
    );
}

#[test]
fn recorder_is_armed_before_the_first_decoded_frame() {
    let root = tempdir().expect("temporary directory");
    let destination = root.path().join("capture.mp4");
    let recorder = Mp4Recorder::start(RecorderConfig::new(destination.clone()))
        .expect("valid destination arms without requiring a decoder frame");

    let status = recorder.status();
    assert_eq!(status.state, RecorderState::Armed);
    assert_eq!(status.frames_accepted, 0);
    assert_eq!(status.frames_written, 0);
    assert!(!destination.exists());
}

#[test]
fn stopping_before_the_first_frame_cancels_without_claiming_a_saved_file() {
    let root = tempdir().expect("temporary directory");
    let destination = root.path().join("capture.mp4");
    let recorder = Mp4Recorder::start(RecorderConfig::new(destination.clone())).expect("armed");

    let status = recorder.stop().expect("clean armed cancellation");
    assert_eq!(status.state, RecorderState::Cancelled);
    assert_eq!(status.frames_written, 0);
    assert!(!destination.exists());
}

#[test]
fn an_invalid_or_changing_frame_terminally_cancels_the_partial_recording() {
    let root = tempdir().expect("temporary directory");
    let destination = root.path().join("capture.mp4");
    let mut recorder = Mp4Recorder::start(RecorderConfig::new(destination.clone())).expect("armed");
    let rgba = vec![127_u8; 64 * 64 * 4];
    recorder
        .submit_padded_rgba(64, 64, 64 * 4, &rgba)
        .expect("first geometry accepted");

    let changed = vec![127_u8; 66 * 64 * 4];
    let error = recorder
        .submit_padded_rgba(66, 64, 66 * 4, &changed)
        .expect_err("geometry changes must terminate the recording");
    assert_eq!(error.code(), "recording.frame_invalid");
    assert_eq!(recorder.status().state, RecorderState::Failed);
    assert_eq!(
        recorder
            .stop()
            .expect_err("failed recording remains failed")
            .code(),
        "recording.frame_invalid"
    );
    assert!(!destination.exists());
    assert!(
        !root
            .path()
            .read_dir()
            .expect("directory")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains("partial"))
    );
}

#[cfg(windows)]
#[test]
fn decoded_rgba_frames_finalize_as_video_only_h264_mp4() {
    let root = tempdir().expect("temporary directory");
    let destination = root.path().join("capture.mp4");
    let mut recorder =
        Mp4Recorder::start(RecorderConfig::new(destination.clone())).expect("valid destination");
    let width = 64_u32;
    let height = 64_u32;
    let row_stride = 256_u32;

    for frame_index in 0_u8..12 {
        let mut rgba = vec![0_u8; (row_stride * height) as usize];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[
                frame_index.saturating_mul(20),
                255_u8.saturating_sub(frame_index.saturating_mul(12)),
                64,
                255,
            ]);
        }
        recorder
            .submit_padded_rgba(width, height, row_stride, &rgba)
            .expect("bounded frame submission");
    }

    let status = recorder.stop().expect("finalized MP4");
    assert_eq!(status.state, RecorderState::Finished);
    assert_eq!(status.frames_accepted, 12);
    assert_eq!(status.frames_written, 12);
    assert_eq!((status.width, status.height), (Some(width), Some(height)));

    let bytes = fs::read(destination).expect("final MP4 exists");
    assert!(bytes.windows(4).any(|window| window == b"ftyp"));
    assert!(bytes.windows(4).any(|window| window == b"moov"));
    assert!(bytes.windows(4).any(|window| window == b"mdat"));
    assert!(bytes.windows(4).any(|window| window == b"avc1"));
    assert!(!bytes.windows(4).any(|window| window == b"mp4a"));
    assert!(
        !root
            .path()
            .read_dir()
            .expect("directory")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains("partial"))
    );
}

#[cfg(windows)]
#[test]
fn final_publication_never_replaces_a_destination_created_after_start() {
    let root = tempdir().expect("temporary directory");
    let destination = root.path().join("capture.mp4");
    let mut recorder = Mp4Recorder::start(RecorderConfig::new(destination.clone()))
        .expect("destination initially absent");
    let width = 64_u32;
    let height = 64_u32;
    let row_stride = width * 4;
    let rgba = vec![96_u8; (row_stride * height) as usize];
    for _ in 0..6 {
        recorder
            .submit_padded_rgba(width, height, row_stride, &rgba)
            .expect("frame accepted");
    }
    fs::write(&destination, b"owner arrived during recording").expect("race destination");

    let error = recorder
        .stop()
        .expect_err("atomic publication must refuse the late destination");
    assert_eq!(error.code(), "recording.output_exists");
    assert_eq!(
        fs::read(&destination).expect("owner file survives"),
        b"owner arrived during recording"
    );
}
