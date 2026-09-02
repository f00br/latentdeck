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
        &D2Controls::default(),
        D2PresetLoops {
            loop_a: true,
            loop_b: false,
        },
        41,
    );

    let encoded = write_deck_preset_json(&preset).expect("serialize preset");
    let text = std::str::from_utf8(&encoded).expect("UTF-8 preset");
    assert!(text.ends_with('\n'));
    assert!(text.contains(r#""schema_version": "2.0.0""#));
    assert!(text.contains(r#""deck_id": "org.latentdeck.deck.d2""#));
    assert!(text.contains(r#""deck_version": "0.2.0""#));
    assert!(!text.contains(r#""deck_type""#));
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
        &Q4Controls::default(),
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
    assert!(String::from_utf8_lossy(&encoded).contains(r#""deck_id": "org.latentdeck.deck.q4""#));
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
        &D2Controls::default(),
        D2PresetLoops {
            loop_a: true,
            loop_b: true,
        },
        7,
    );
    let encoded =
        String::from_utf8(write_deck_preset_json(&valid).expect("preset JSON")).expect("UTF-8");

    let duplicate = encoded.replacen(
        r#""schema_version": "2.0.0","#,
        r#""schema_version": "2.0.0", "schema_version": "2.0.0","#,
        1,
    );
    assert_eq!(
        parse_deck_preset_json(duplicate.as_bytes())
            .expect_err("duplicate key")
            .code(),
        "preset.duplicate_key"
    );

    let unknown = encoded.replacen(
        r#""schema_version": "2.0.0","#,
        r#""schema_version": "2.0.0", "surprise": true,"#,
        1,
    );
    assert_eq!(
        parse_deck_preset_json(unknown.as_bytes())
            .expect_err("unknown field")
            .code(),
        "preset.invalid_json"
    );

    let unsupported = encoded.replace(DECK_PRESET_SCHEMA_VERSION, "3.0.0");
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
fn legacy_d2_and_q4_load_deterministically_and_next_write_is_v2() {
    let d2 = serde_json::json!({
        "deck_type": "LD-D2",
        "schema_version": "0.1.0",
        "active_collection_id": "latentdeck.virtual.all",
        "slots": {"a": identity('a', 1), "b": identity('b', 2)},
        "controls": D2Controls::default(),
        "loops": {"loop_a": true, "loop_b": false},
        "seed": 44
    });
    let d2_v2 = parse_deck_preset_json(
        serde_json::to_string(&d2)
            .expect("legacy D2 JSON")
            .as_bytes(),
    )
    .expect("migrate legacy D2");
    assert_eq!(d2_v2.deck_id, "org.latentdeck.deck.d2");
    assert_eq!(d2_v2.deck_version, "0.2.0");
    assert_eq!(d2_v2.roles.get("carrier"), Some(&1));
    assert_eq!(d2_v2.roles.get("donor"), Some(&2));
    assert!(!d2_v2.controls.contains_key("routing"));

    let q4 = serde_json::json!({
        "deck_type": "LD-Q4",
        "schema_version": "0.1.0",
        "active_collection_id": "latentdeck.virtual.unassigned",
        "slots": {
            "a": identity('a', 1),
            "b": identity('b', 2),
            "c": identity('c', 3),
            "d": identity('d', 4)
        },
        "controls": Q4Controls::default(),
        "routing": {"carrier": "C", "donor_b": "A", "donor_c": "D", "donor_d": "B"},
        "loops": {"loop_a": true, "loop_b": false, "loop_c": true, "loop_d": false},
        "seed": 9
    });
    let q4_v2 = parse_deck_preset_json(
        serde_json::to_string(&q4)
            .expect("legacy Q4 JSON")
            .as_bytes(),
    )
    .expect("migrate legacy Q4");
    assert_eq!(q4_v2.deck_id, "org.latentdeck.deck.q4");
    assert_eq!(q4_v2.roles.get("carrier"), Some(&3));
    assert_eq!(q4_v2.roles.get("donor_b"), Some(&1));
    assert_eq!(q4_v2.roles.get("donor_c"), Some(&4));
    assert_eq!(q4_v2.roles.get("donor_d"), Some(&2));

    for migrated in [&d2_v2, &q4_v2] {
        let next = String::from_utf8(write_deck_preset_json(migrated).expect("write v2"))
            .expect("UTF-8 v2");
        assert!(next.contains(r#""schema_version": "2.0.0""#));
        assert!(!next.contains(r#""deck_type""#));
    }
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
        &D2Controls::default(),
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
