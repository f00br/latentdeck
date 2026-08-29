from __future__ import annotations

from copy import deepcopy

import pytest
from latentdeck_codec_host.protocol import ProtocolError, SequenceValidator

SESSION_ID = "9ca8c228-04c7-4b59-909f-6fbef591a43e"
MESSAGE_ID = "f78e4568-f58e-4fe7-888a-5cf1cc2db76b"
CAPTURE_ID = "33333333-3333-4333-8333-333333333333"


def command_envelope(name: str, payload: dict[str, object]) -> dict[str, object]:
    return {
        "protocol": "latentdeck.worker",
        "protocol_version": 1,
        "session_id": SESSION_ID,
        "sequence": 1,
        "message_id": MESSAGE_ID,
        "sender_uptime_ns": 1,
        "message": {"kind": "command", "body": {"name": name, "payload": payload}},
    }


def source(slot: str) -> dict[str, object]:
    identities = {
        "a": "11111111-1111-4111-8111-111111111111",
        "b": "22222222-2222-4222-8222-222222222222",
        "c": "33333333-3333-4333-8333-333333333333",
        "d": "44444444-4444-4444-8444-444444444444",
    }
    return {
        "cartridge_path": f"C:/private/{slot}.lc",
        "cartridge_id": identities[slot],
        "expected_archive_sha256": slot * 64,
    }


def roles() -> dict[str, object]:
    return {"carrier": "A", "donor_b": "B", "donor_c": "C", "donor_d": "D"}


def controls() -> dict[str, object]:
    return {
        "algorithm": "LINEAR",
        "interaction": 0.0,
        "mode": "HYBRIDIZE",
        "preserve": 0.55,
        "influence_mode": "MANUAL",
        "donor_weight_b": 1.0,
        "donor_weight_c": 1.0,
        "donor_weight_d": 1.0,
        "triangle_x": 0.5,
        "triangle_y": 1.0 / 3.0,
        "xs5_routing": "TOPK",
        "temperature": 0.12,
        "top_k": 8,
        "sinkhorn_iterations": 5,
        "chaos": 0.0,
    }


def transport() -> dict[str, object]:
    return {
        "playing_a": True,
        "playing_b": True,
        "playing_c": True,
        "playing_d": True,
        "loop_a": True,
        "loop_b": True,
        "loop_c": True,
        "loop_d": True,
    }


def load_payload() -> dict[str, object]:
    return {
        "deck_id": "main-q4",
        "operator_id": "org.latentdeck.builtin.ld_q4",
        "operator_version": "0.1.0",
        "source_a": source("a"),
        "source_b": source("b"),
        "source_c": source("c"),
        "source_d": source("d"),
        "roles": roles(),
        "controls": controls(),
        "transport": transport(),
        "seed": 42,
        "stream_generation": 1,
    }


def accepted(name: str, payload: dict[str, object]) -> dict[str, object]:
    return SequenceValidator(SESSION_ID).validate_command(command_envelope(name, payload))


def test_q4_load_and_all_runtime_commands_use_closed_shapes() -> None:
    assert accepted("deck.q4.load", load_payload())["name"] == "deck.q4.load"

    commands = {
        "deck.q4.process_slot": {
            "deck_id": "main-q4",
            "deck_revision": 1,
            "stream_generation": 1,
        },
        "deck.q4.reset": {
            "deck_id": "main-q4",
            "deck_revision": 1,
            "new_stream_generation": 2,
        },
        "deck.q4.restart": {"deck_id": "main-q4", "deck_revision": 1},
        "deck.q4.controls.set": {
            "deck_id": "main-q4",
            "deck_revision": 1,
            "controls": controls(),
        },
        "deck.q4.roles.set": {
            "deck_id": "main-q4",
            "deck_revision": 1,
            "roles": roles(),
        },
        "deck.q4.transport.set": {
            "deck_id": "main-q4",
            "deck_revision": 1,
            "transport": transport(),
        },
        "deck.q4.seed.set": {"deck_id": "main-q4", "deck_revision": 1, "seed": 99},
        "deck.q4.status": {},
    }
    for name, payload in commands.items():
        assert accepted(name, payload)["name"] == name

    unknown = deepcopy(load_payload())
    unknown["hidden_rgb_fallback"] = True
    with pytest.raises(ProtocolError, match="closed schema"):
        accepted("deck.q4.load", unknown)


def test_q4_load_requires_four_exact_worker_bindings() -> None:
    for field in ("source_a", "source_b", "source_c", "source_d"):
        payload = deepcopy(load_payload())
        binding = payload[field]
        assert isinstance(binding, dict)
        binding["path_from_webview"] = "forbidden.lc"
        with pytest.raises(ProtocolError, match="closed schema"):
            accepted("deck.q4.load", payload)

    payload = deepcopy(load_payload())
    source_c = payload["source_c"]
    assert isinstance(source_c, dict)
    source_c["expected_archive_sha256"] = "C" * 64
    with pytest.raises(ProtocolError, match="canonical"):
        accepted("deck.q4.load", payload)


def test_q4_roles_are_an_exact_permutation_and_reject_hidden_carrier_fields() -> None:
    payload = deepcopy(load_payload())
    role_block = payload["roles"]
    assert isinstance(role_block, dict)
    role_block["donor_b"] = "A"
    with pytest.raises(ProtocolError, match="permutation"):
        accepted("deck.q4.load", payload)

    payload = deepcopy(load_payload())
    role_block = payload["roles"]
    assert isinstance(role_block, dict)
    role_block["hidden_carrier"] = "D"
    with pytest.raises(ProtocolError, match="closed schema"):
        accepted("deck.q4.load", payload)


def test_q4_controls_reject_nonfinite_hidden_downscale_and_invalid_influence() -> None:
    cases: list[tuple[str, object, str]] = [
        ("interaction", float("nan"), "finite bound"),
        ("top_k", 65, "integer bound"),
        ("sinkhorn_iterations", 1, "integer bound"),
    ]
    for field, value, message in cases:
        payload = deepcopy(load_payload())
        control_block = payload["controls"]
        assert isinstance(control_block, dict)
        control_block[field] = value
        with pytest.raises(ProtocolError, match=message):
            accepted("deck.q4.load", payload)

    payload = deepcopy(load_payload())
    control_block = payload["controls"]
    assert isinstance(control_block, dict)
    control_block["hidden_downscale"] = 0.5
    with pytest.raises(ProtocolError, match="closed schema"):
        accepted("deck.q4.load", payload)

    payload = deepcopy(load_payload())
    control_block = payload["controls"]
    assert isinstance(control_block, dict)
    control_block.update({"donor_weight_b": 0.0, "donor_weight_c": 0.0, "donor_weight_d": 0.0})
    with pytest.raises(ProtocolError, match="positive"):
        accepted("deck.q4.load", payload)

    payload = deepcopy(load_payload())
    control_block = payload["controls"]
    assert isinstance(control_block, dict)
    control_block.update({"influence_mode": "TRIANGLE", "triangle_x": 0.1, "triangle_y": 0.9})
    with pytest.raises(ProtocolError, match="outside"):
        accepted("deck.q4.load", payload)


def test_q4_transport_is_eight_exact_booleans() -> None:
    payload = deepcopy(load_payload())
    transport_block = payload["transport"]
    assert isinstance(transport_block, dict)
    transport_block["playing_c"] = 1
    with pytest.raises(ProtocolError, match="boolean"):
        accepted("deck.q4.load", payload)

    payload = deepcopy(load_payload())
    transport_block = payload["transport"]
    assert isinstance(transport_block, dict)
    del transport_block["loop_d"]
    with pytest.raises(ProtocolError, match="closed schema"):
        accepted("deck.q4.load", payload)


def test_q4_capture_commands_are_closed_bounded_and_canonical() -> None:
    start = {
        "deck_id": "main-q4",
        "deck_revision": 1,
        "capture_id": CAPTURE_ID,
        "mode": "snapshot",
        "temporary_root": "C:/private/capture",
        "max_latent_slots": 128,
        "max_visual_bytes": 16 * 1024 * 1024,
    }
    assert accepted("deck.q4.capture.start", start)["name"] == "deck.q4.capture.start"
    identity = {"deck_id": "main-q4", "deck_revision": 1, "capture_id": CAPTURE_ID}
    assert accepted("deck.q4.capture.stop", identity)["name"] == "deck.q4.capture.stop"
    assert accepted("deck.q4.capture.status", identity)["name"] == "deck.q4.capture.status"

    cases = [
        ("mode", "SNAPSHOT", "closed enum"),
        ("max_latent_slots", 1, "integer bound"),
        ("max_latent_slots", 1_048_577, "integer bound"),
        ("max_visual_bytes", 0, "integer bound"),
    ]
    for field, value, message in cases:
        invalid = dict(start)
        invalid[field] = value
        with pytest.raises(ProtocolError, match=message):
            accepted("deck.q4.capture.start", invalid)

    unknown = dict(start)
    unknown["output_path"] = "forbidden.lc"
    with pytest.raises(ProtocolError, match="closed schema"):
        accepted("deck.q4.capture.start", unknown)


def test_q4_seed_and_generation_use_exact_nonnegative_bounds() -> None:
    payload = deepcopy(load_payload())
    payload["seed"] = 9_007_199_254_740_992
    with pytest.raises(ProtocolError, match="integer bound"):
        accepted("deck.q4.load", payload)

    for name, payload in (
        (
            "deck.q4.process_slot",
            {"deck_id": "main-q4", "deck_revision": 1, "stream_generation": 0},
        ),
        (
            "deck.q4.reset",
            {"deck_id": "main-q4", "deck_revision": 1, "new_stream_generation": 0},
        ),
    ):
        with pytest.raises(ProtocolError, match="positive"):
            accepted(name, payload)
