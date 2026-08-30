use latentdeck_control::{
    D2Controls, D2PresetLoops, DECK_PRESET_SCHEMA_VERSION, DeckPresetDocument,
    PresetCartridgeIdentity, Q4Controls, Q4PresetLoops, Q4Roles, WireUuid, parse_deck_preset_json,
    write_deck_preset_json,
};
use uuid::Uuid;

fn identity(marker: char, uuid_tail: u128) -> PresetCartridgeIdentity {
    PresetCartridgeIdentity {
        cartridge_id: WireUuid::from_uuid(Uuid::from_u128(uuid_tail)),
        archive_sha256: marker.to_string().repeat(64),
    }
}

#[test]
fn d2_preset_roundtrips_as_versioned_path_free_json() {
    let preset = DeckPresetDocument::d2(
        "latentdeck.virtual.all".to_owned(),
        identity('a', 1),
        identity('b', 2),
        D2Controls::default(),
        D2PresetLoops {
            loop_a: true,
            loop_b: false,
        },
        41,
    );

    let encoded = write_deck_preset_json(&preset).expect("serialize preset");
    let text = std::str::from_utf8(&encoded).expect("UTF-8 preset");
    assert!(text.ends_with('\n'));
    assert!(text.contains(r#""schema_version": "0.1.0""#));
    assert!(text.contains(r#""deck_type": "LD-D2""#));
    assert!(!text.contains(r"cartridge_path"));
    assert_eq!(
        parse_deck_preset_json(&encoded).expect("parse preset"),
        preset
    );
}

#[test]
fn q4_preset_allows_deliberately_reused_cartridge_identities() {
    let reused = identity('c', 3);
    let preset = DeckPresetDocument::q4(
        "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        [reused.clone(), identity('d', 4), identity('e', 5), reused],
        Q4Controls::default(),
        Q4Roles::default(),
        Q4PresetLoops {
            loop_a: true,
            loop_b: true,
            loop_c: false,
            loop_d: true,
        },
        9_007_199_254_740_991,
    );

    let encoded = write_deck_preset_json(&preset).expect("serialize preset");
    assert!(String::from_utf8_lossy(&encoded).contains(r#""deck_type": "LD-Q4""#));
    assert_eq!(
        parse_deck_preset_json(&encoded).expect("parse preset"),
        preset
    );
}

#[test]
fn preset_parser_rejects_duplicates_unknown_fields_versions_and_invalid_identity() {
    let valid = DeckPresetDocument::d2(
        "latentdeck.virtual.unassigned".to_owned(),
        identity('a', 1),
        identity('b', 2),
        D2Controls::default(),
        D2PresetLoops {
            loop_a: true,
            loop_b: true,
        },
        7,
    );
    let encoded =
        String::from_utf8(write_deck_preset_json(&valid).expect("preset JSON")).expect("UTF-8");

    let duplicate = encoded.replacen(
        r#""schema_version": "0.1.0","#,
        r#""schema_version": "0.1.0", "schema_version": "0.1.0","#,
        1,
    );
    assert_eq!(
        parse_deck_preset_json(duplicate.as_bytes())
            .expect_err("duplicate key")
            .code(),
        "preset.duplicate_key"
    );

    let unknown = encoded.replacen(
        r#""schema_version": "0.1.0","#,
        r#""schema_version": "0.1.0", "surprise": true,"#,
        1,
    );
    assert_eq!(
        parse_deck_preset_json(unknown.as_bytes())
            .expect_err("unknown field")
            .code(),
        "preset.invalid_json"
    );

    let unsupported = encoded.replace(DECK_PRESET_SCHEMA_VERSION, "0.2.0");
    assert_eq!(
        parse_deck_preset_json(unsupported.as_bytes())
            .expect_err("unsupported version")
            .code(),
        "preset.unsupported_version"
    );

    let invalid_hash = encoded.replacen(&"a".repeat(64), &"A".repeat(64), 1);
    assert_eq!(
        parse_deck_preset_json(invalid_hash.as_bytes())
            .expect_err("uppercase hash")
            .code(),
        "preset.invalid_field"
    );
}

#[test]
fn preset_writer_rejects_unsafe_semantics_before_emitting_bytes() {
    let nil = PresetCartridgeIdentity {
        cartridge_id: WireUuid::from_uuid(Uuid::nil()),
        archive_sha256: "a".repeat(64),
    };
    let preset = DeckPresetDocument::d2(
        "not a collection".to_owned(),
        nil,
        identity('b', 2),
        D2Controls::default(),
        D2PresetLoops {
            loop_a: true,
            loop_b: true,
        },
        0,
    );

    assert_eq!(
        write_deck_preset_json(&preset)
            .expect_err("invalid preset")
            .code(),
        "preset.invalid_field"
    );
}
