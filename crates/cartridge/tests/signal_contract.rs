mod support;

use latentdeck_cartridge::{manifest::Rational, signal::validate_codec_neutral_signal_geometry};

#[test]
fn accepts_generic_geometry_without_applying_h3_semantics() {
    let payload = support::synthetic_non_h3_payload();
    let manifest = support::synthetic_non_h3_manifest(&payload);

    let geometry =
        validate_codec_neutral_signal_geometry(&manifest).expect("generic signal geometry");

    assert_eq!(geometry.batch, 1);
    assert_eq!(geometry.latent_channels, 7);
    assert_eq!(geometry.latent_slots, 1);
    assert_eq!(geometry.latent_height, 3);
    assert_eq!(geometry.latent_width, 1);
    assert_eq!(geometry.decoded_frame_count, 1);
    assert_eq!(geometry.decoded_width, 3);
    assert_eq!(geometry.decoded_height, 1);
}

#[test]
fn rejects_non_unit_batch_and_contradictory_decoded_duration() {
    let payload = support::synthetic_non_h3_payload();
    let mut malformed_shape = support::synthetic_non_h3_manifest(&payload);
    malformed_shape.tensors[0].shape[0] = 2;
    let shape_error = validate_codec_neutral_signal_geometry(&malformed_shape)
        .expect_err("batch must remain one");
    assert_eq!(shape_error.code(), "tensor_shape_invalid");

    let mut malformed_timing = support::synthetic_non_h3_manifest(&payload);
    malformed_timing.timing.decoded_video.duration = Rational {
        numerator: 2,
        denominator: 1,
    };
    let timing_error = validate_codec_neutral_signal_geometry(&malformed_timing)
        .expect_err("duration must match frame count and rate");
    assert_eq!(timing_error.code(), "timing_mismatch");
}
