use latentdeck_core::product_version;

#[test]
fn product_version_comes_from_cargo_metadata() {
    assert_eq!(product_version(), "0.1.0");
}
