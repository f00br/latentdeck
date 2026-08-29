use latentdeck_cartridge::limits::ValidationLimits;
use latentdeck_cartridge::preview::inspect_webp;

fn vp8x_preview(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&22_u32.to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(b"VP8X");
    bytes.extend_from_slice(&10_u32.to_le_bytes());
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    let width_minus_one = width - 1;
    let height_minus_one = height - 1;
    bytes.extend_from_slice(&width_minus_one.to_le_bytes()[..3]);
    bytes.extend_from_slice(&height_minus_one.to_le_bytes()[..3]);
    bytes
}

#[test]
fn reads_bounded_vp8x_canvas_dimensions() {
    let bytes = vp8x_preview(448, 800);
    let info = inspect_webp(&bytes, &ValidationLimits::default()).expect("valid WebP envelope");
    assert_eq!(info.width, 448);
    assert_eq!(info.height, 800);
}

#[test]
fn rejects_trailing_bytes_and_oversized_canvas() {
    let mut trailing = vp8x_preview(448, 800);
    trailing.push(0);
    let error = inspect_webp(&trailing, &ValidationLimits::default()).expect_err("trailing bytes");
    assert_eq!(error.code(), "manifest_invalid");

    let oversized = vp8x_preview(4097, 1);
    let error = inspect_webp(&oversized, &ValidationLimits::default()).expect_err("axis ceiling");
    assert_eq!(error.code(), "runtime_limit_exceeded");
}
