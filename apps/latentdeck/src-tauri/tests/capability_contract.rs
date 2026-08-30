use serde_json::Value;

#[test]
fn main_window_can_import_cartridges_and_roundtrip_presets_without_broad_file_access() {
    let capability: Value = serde_json::from_str(include_str!("../capabilities/main.json"))
        .expect("LatentDeck capability JSON");

    assert_eq!(capability["identifier"], "main-window");
    assert_eq!(capability["windows"], serde_json::json!(["main"]));
    assert_eq!(
        capability["permissions"],
        serde_json::json!(["core:default", "dialog:allow-open"])
    );
}
