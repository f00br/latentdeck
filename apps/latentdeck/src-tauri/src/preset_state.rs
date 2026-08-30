//! Native file boundary for portable Deck presets.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

use atomicwrites::move_atomic;
use latentdeck_control::{
    DeckPresetDocument, MAX_DECK_PRESET_BYTES, parse_deck_preset_json, write_deck_preset_json,
};
use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt as _;

use crate::library_state::CommandError;

const MAX_PARTIAL_CANDIDATES: u32 = 100;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PresetSaveView {
    saved: bool,
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri owns command parameters.
pub(crate) fn deck_preset_save(
    app: AppHandle,
    preset: DeckPresetDocument,
) -> Result<Option<PresetSaveView>, CommandError> {
    let suggested_name = match &preset {
        DeckPresetDocument::D2 { .. } => "latentdeck-d2-preset.json",
        DeckPresetDocument::Q4 { .. } => "latentdeck-q4-preset.json",
    };
    let selected = app
        .dialog()
        .file()
        .add_filter("LatentDeck Preset", &["json"])
        .set_file_name(suggested_name)
        .blocking_save_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected.into_path().map_err(|_| {
        command_error(
            "preset.output_path_invalid",
            "The native save dialog did not return a usable preset path.",
        )
    })?;
    let output = validate_output_path(path)?;
    let bytes = write_deck_preset_json(&preset).map_err(|error| {
        command_error(
            error.code(),
            format!("Deck preset is invalid: {}", error.detail()),
        )
    })?;
    write_atomic(&output, &bytes)?;
    Ok(Some(PresetSaveView { saved: true }))
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri owns command parameters.
pub(crate) fn deck_preset_load(app: AppHandle) -> Result<Option<DeckPresetDocument>, CommandError> {
    let selected = app
        .dialog()
        .file()
        .add_filter("LatentDeck Preset", &["json"])
        .blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected.into_path().map_err(|_| {
        command_error(
            "preset.input_path_invalid",
            "The native open dialog did not return a usable preset path.",
        )
    })?;
    load_file(&path).map(Some)
}

fn validate_output_path(path: PathBuf) -> Result<PathBuf, CommandError> {
    if !path.is_absolute() {
        return Err(command_error(
            "preset.output_path_invalid",
            "Deck preset output must be an absolute path.",
        ));
    }
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("json"))
    {
        return Err(command_error(
            "preset.output_path_invalid",
            "Deck presets must use the .json extension.",
        ));
    }
    if path.exists() {
        return Err(command_error(
            "target.exists",
            "Deck preset save never overwrites an existing file.",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        command_error(
            "preset.output_path_invalid",
            "Deck preset output has no parent directory.",
        )
    })?;
    if !parent.is_dir() {
        return Err(command_error(
            "preset.output_path_invalid",
            "Deck preset output directory does not exist.",
        ));
    }
    Ok(path)
}

fn load_file(path: &Path) -> Result<DeckPresetDocument, CommandError> {
    if !path.is_absolute()
        || !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("json"))
    {
        return Err(command_error(
            "preset.input_path_invalid",
            "Deck preset input must be an absolute .json file.",
        ));
    }
    let metadata = fs::metadata(path).map_err(|_| {
        command_error(
            "preset.input_unavailable",
            "The selected Deck preset is unavailable.",
        )
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_DECK_PRESET_BYTES as u64 {
        return Err(command_error(
            "preset.too_large",
            "The selected Deck preset is empty, not a regular file, or exceeds its byte bound.",
        ));
    }
    let mut input = File::open(path).map_err(|_| {
        command_error(
            "preset.input_unavailable",
            "The selected Deck preset could not be opened.",
        )
    })?;
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        command_error(
            "preset.too_large",
            "The selected Deck preset length cannot be represented safely.",
        )
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    std::io::Read::by_ref(&mut input)
        .take((MAX_DECK_PRESET_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            command_error(
                "preset.input_unavailable",
                "The selected Deck preset could not be read.",
            )
        })?;
    if bytes.len() > MAX_DECK_PRESET_BYTES {
        return Err(command_error(
            "preset.too_large",
            "The selected Deck preset grew beyond its byte bound while reading.",
        ));
    }
    parse_deck_preset_json(&bytes).map_err(|error| {
        command_error(
            error.code(),
            format!("Deck preset is invalid: {}", error.detail()),
        )
    })
}

fn write_atomic(output: &Path, bytes: &[u8]) -> Result<(), CommandError> {
    let parent = output.parent().ok_or_else(|| {
        command_error(
            "preset.output_path_invalid",
            "Deck preset output has no parent directory.",
        )
    })?;
    let file_name = output
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            command_error(
                "preset.output_path_invalid",
                "Deck preset output name is not portable text.",
            )
        })?;
    for attempt in 1..=MAX_PARTIAL_CANDIDATES {
        let partial = parent.join(format!(
            ".{file_name}.partial-{}-{attempt}",
            std::process::id()
        ));
        let mut file = match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&partial)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                return Err(command_error(
                    "preset.write_failed",
                    "Deck preset temporary file could not be created.",
                ));
            }
        };
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            move_atomic(&partial, output)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&partial);
            return Err(command_error(
                "preset.write_failed",
                "Deck preset could not be finalized atomically.",
            ));
        }
        return Ok(());
    }
    Err(command_error(
        "preset.write_failed",
        "Deck preset temporary-file candidate limit was reached.",
    ))
}

fn command_error(code: impl Into<String>, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

#[cfg(test)]
mod tests {
    use latentdeck_control::{D2Controls, D2PresetLoops, PresetCartridgeIdentity, WireUuid};
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::*;

    fn preset() -> DeckPresetDocument {
        let identity = |marker: char, value: u128| PresetCartridgeIdentity {
            cartridge_id: WireUuid::from_uuid(Uuid::from_u128(value)),
            archive_sha256: marker.to_string().repeat(64),
        };
        DeckPresetDocument::d2(
            "latentdeck.virtual.all".to_owned(),
            identity('a', 1),
            identity('b', 2),
            D2Controls::default(),
            D2PresetLoops {
                loop_a: true,
                loop_b: false,
            },
            17,
        )
    }

    #[test]
    fn preset_file_roundtrip_is_atomic_and_never_overwrites() {
        let temporary = tempdir().expect("temporary directory");
        let output = temporary.path().join("deck.json");
        let bytes = write_deck_preset_json(&preset()).expect("serialize preset");

        write_atomic(&output, &bytes).expect("atomic write");
        assert_eq!(load_file(&output).expect("load preset"), preset());
        assert!(write_atomic(&output, &bytes).is_err());
        assert_eq!(fs::read(&output).expect("read output"), bytes);
        assert!(
            fs::read_dir(temporary.path())
                .expect("read directory")
                .all(|entry| !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .contains("partial"))
        );
    }

    #[test]
    fn preset_file_boundary_rejects_wrong_extension_existing_and_oversized_input() {
        let temporary = tempdir().expect("temporary directory");
        let wrong_extension = temporary.path().join("deck.txt");
        fs::write(&wrong_extension, b"{}").expect("write wrong extension");
        assert!(load_file(&wrong_extension).is_err());

        let existing = temporary.path().join("existing.json");
        fs::write(&existing, b"owned").expect("write existing");
        assert!(validate_output_path(existing).is_err());

        let oversized = temporary.path().join("oversized.json");
        fs::write(&oversized, vec![b' '; MAX_DECK_PRESET_BYTES + 1]).expect("write oversized");
        assert!(load_file(&oversized).is_err());
    }
}
