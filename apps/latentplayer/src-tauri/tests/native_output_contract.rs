// Compile and exercise the native-output module before it is wired into the
// Player actor. Keeping this explicit avoids coupling the presentation slice
// to the concurrently evolving Tauri command entrypoint.
#[allow(dead_code)] // Public actor API is compiled here before main.rs integration.
#[path = "../src/native_output.rs"]
mod native_output;
