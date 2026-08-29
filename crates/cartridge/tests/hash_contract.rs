use std::io::Cursor;

use latentdeck_cartridge::hash::{Sha256Hash, hash_reader};

#[test]
fn sha256_is_streamed_and_rendered_as_lowercase_hex() {
    let mut input = Cursor::new(b"abc");
    let measured = hash_reader(&mut input).expect("stream hash");
    assert_eq!(measured.byte_length, 3);
    assert_eq!(
        measured.sha256,
        Sha256Hash::parse("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
            .expect("known SHA-256")
    );
    assert_eq!(
        measured.sha256.to_string(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn sha256_parser_rejects_noncanonical_text() {
    let error =
        Sha256Hash::parse("BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD")
            .expect_err("uppercase hash");
    assert_eq!(error.code(), "manifest_invalid");
}
