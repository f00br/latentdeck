use serde_json::Value;

#[test]
fn main_window_can_open_cartridges_and_decoder_weights_without_broad_file_access() {
    let capability: Value = serde_json::from_str(include_str!("../capabilities/main.json"))
        .expect("LatentPlayer capability JSON");

    assert_eq!(capability["identifier"], "main-window");
    assert_eq!(capability["windows"], serde_json::json!(["main"]));
    assert_eq!(
        capability["permissions"],
        serde_json::json!(["core:default", "dialog:allow-open"])
    );
}
