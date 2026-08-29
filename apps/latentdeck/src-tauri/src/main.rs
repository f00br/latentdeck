#[tauri::command]
const fn product_version() -> &'static str {
    latentdeck_core::product_version()
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![product_version])
        .run(tauri::generate_context!())
        .expect("LatentDeck application runtime failed");
}
