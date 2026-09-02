use latentdeck_control::{FramingError, decode_envelope};
use serde_json::json;

fn legacy_deck_command(name: &str) -> Vec<u8> {
    rmp_serde::to_vec_named(&json!({
        "protocol": "latentdeck.worker",
        "protocol_version": 1,
        "session_id": "11111111-1111-4111-8111-111111111111",
        "sequence": 1,
        "message_id": "22222222-2222-4222-8222-222222222222",
        "sender_uptime_ns": 1,
        "message": {
            "kind": "command",
            "body": {
                "name": name,
                "payload": {}
            }
        }
    }))
    .expect("test envelope must encode")
}

#[test]
fn protocol1_rejects_legacy_deck_command_names() {
    for name in ["deck.d2.status", "deck.q4.status"] {
        let error = decode_envelope(&legacy_deck_command(name))
            .expect_err("Protocol 1 is Player-only and must reject Deck commands");
        assert!(matches!(error, FramingError::Decode(_)), "{name}: {error}");
    }
}
