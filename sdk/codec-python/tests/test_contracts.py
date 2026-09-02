from __future__ import annotations

import uuid
from dataclasses import replace

import pytest

from latentdeck_codec_sdk import (
    Capability,
    CapturePayload,
    CaptureRequest,
    CodecDescriptor,
    CodecSdkError,
    DecodedAbi,
    ProfileKey,
    ProfileReceipt,
    RawImportArtifact,
    RawImportMetadata,
    RawImportPreflight,
    RawImportPreflightRequest,
    RawImportStageRequest,
    RawImportTensor,
    SignalGeometry,
    TensorAbi,
    TensorAccessDescriptor,
    validate_codec_v2_descriptor,
    validate_profile_receipt,
)


def profile() -> ProfileKey:
    return ProfileKey("synthetic", "test_latent", "0.1.0")


def descriptor() -> CodecDescriptor:
    return CodecDescriptor(
        pack_id="org.example.synthetic",
        pack_version="0.2.0",
        adapter_id="org.example.synthetic.adapter",
        adapter_version="0.2.0",
        host_api_version="2.0",
        capabilities=(
            Capability.PLAYER,
            Capability.REALTIME,
            Capability.RESAMPLE,
            Capability.SNAPSHOT_CAPTURE,
            Capability.LIVE_CAPTURE,
        ),
        profiles=(profile(),),
    )


def receipt() -> ProfileReceipt:
    return ProfileReceipt(
        receipt_id=uuid.UUID("10000000-0000-4000-8000-000000000001"),
        cartridge_id=uuid.UUID("10000000-0000-4000-8000-000000000002"),
        archive_sha256="a" * 64,
        payload_sha256="b" * 64,
        pack_id="org.example.synthetic",
        pack_version="0.2.0",
        adapter_id="org.example.synthetic.adapter",
        adapter_version="0.2.0",
        profile_key=profile(),
        signal_geometry=SignalGeometry(
            channels=4,
            latent_height=8,
            latent_width=8,
            decoded_height=64,
            decoded_width=64,
            frame_rate_numerator=24,
            frame_rate_denominator=1,
            timing_contract="synthetic_causal",
            timing_contract_version="0.1.0",
        ),
        tensor_abi=TensorAbi(
            python_version="3.13",
            torch_version="2.13.0+cu130",
            dtype="float16",
            shape=(1, 4, 1, 8, 8),
            device="cuda",
        ),
        decoded_abi=DecodedAbi(),
        capabilities=(Capability.PLAYER, Capability.REALTIME),
        estimated_host_bytes=4096,
        estimated_device_bytes=8192,
    )


def test_full_codec_v2_descriptor_and_bound_receipt_validate() -> None:
    assert validate_codec_v2_descriptor(descriptor()) == descriptor()
    assert validate_profile_receipt(receipt(), descriptor()) == receipt()


def test_full_codec_v2_descriptor_rejects_a_missing_required_capability() -> None:
    invalid = replace(
        descriptor(),
        capabilities=tuple(
            value for value in descriptor().capabilities if value is not Capability.LIVE_CAPTURE
        ),
    )
    with pytest.raises(CodecSdkError, match="codec.capability_missing"):
        validate_codec_v2_descriptor(invalid)


def test_receipt_rejects_a_descriptor_identity_mismatch() -> None:
    mismatched = replace(receipt(), adapter_version="0.2.1")
    with pytest.raises(CodecSdkError, match="profile.identity_mismatch"):
        validate_profile_receipt(mismatched, descriptor())


def test_tensor_abi_is_exactly_five_dimensional_and_contiguous() -> None:
    invalid = TensorAbi(
        python_version="3.13",
        torch_version="2.13.0+cu130",
        dtype="float16",
        shape=(2, 4, 1, 8, 8),
        device="cuda",
        contiguous=True,
    )
    with pytest.raises(CodecSdkError, match="tensor.shape"):
        invalid.validate()


def test_tensor_access_descriptor_binds_exact_storage_bytes_without_offsets() -> None:
    descriptor = TensorAccessDescriptor("video", "F16", (1, 24, 7, 2, 3), 2016)
    descriptor.validate()
    assert not hasattr(descriptor, "offset")

    with pytest.raises(CodecSdkError, match="tensor.storage_length"):
        replace(descriptor, byte_length=2015).validate()


def test_capture_contract_binds_host_staging_and_decoded_timing(tmp_path) -> None:
    capture_id = uuid.UUID("10000000-0000-4000-8000-000000000008")
    staging_root = (tmp_path / "capture-staging").resolve()
    payload_path = staging_root / "capture.safetensors"
    request = CaptureRequest(
        capture_id=capture_id,
        mode="snapshot",
        staging_root=str(staging_root),
        maximum_latent_slots=128,
        maximum_visual_bytes=64 * 1024 * 1024,
    )
    request.validate()
    payload = CapturePayload(
        capture_id=capture_id,
        payload_path=str(payload_path),
        payload_sha256="a" * 64,
        payload_byte_length=4096,
        latent_slots=2,
        decoded_frame_count=5,
    )
    payload.validate()

    with pytest.raises(CodecSdkError, match="capture.staging_root"):
        replace(request, staging_root="relative/capture").validate()
    with pytest.raises(CodecSdkError, match="decoded_frame_count"):
        replace(payload, decoded_frame_count=0).validate()


def raw_import_metadata() -> RawImportMetadata:
    return RawImportMetadata(
        profile_key=ProfileKey("minimax_h3", "h3_av_latent", "0.1.0"),
        payload_entry="payloads/h3.safetensors",
        payload_media_type="application/vnd.safetensors",
        tensors=(
            RawImportTensor(
                stream="visual",
                name="video",
                storage_dtype="F16",
                runtime_dtype="F16",
                shape=(1, 24, 7, 8, 8),
            ),
        ),
        timing_contract="minimax_h3_causal",
        timing_contract_version="0.1.0",
        decoded_width=128,
        decoded_height=128,
        decoded_frame_count=22,
        frame_rate_numerator=24,
        frame_rate_denominator=1,
        duration_numerator=11,
        duration_denominator=12,
        audio_policy="source_absent",
    )


def test_optional_raw_import_contract_is_cpu_only_path_bounded_and_receipted(tmp_path) -> None:
    import_id = uuid.UUID("10000000-0000-4000-8000-000000000010")
    receipt_id = uuid.UUID("10000000-0000-4000-8000-000000000011")
    source = tmp_path / "source.safetensors"
    staging_root = tmp_path / "staging"
    staged = staging_root / "staged.safetensors"
    request = RawImportPreflightRequest(import_id, str(source), 64 * 1024 * 1024)
    request.validate()
    preflight = RawImportPreflight(
        receipt_id=receipt_id,
        import_id=import_id,
        pack_id="org.latentdeck.h3",
        pack_version="0.2.0",
        adapter_id="org.latentdeck.h3",
        adapter_version="0.2.0",
        source_sha256="a" * 64,
        source_byte_length=4096,
        metadata=raw_import_metadata(),
    )
    preflight.validate()
    stage = RawImportStageRequest(preflight, str(staging_root))
    stage.validate()
    artifact = RawImportArtifact(
        receipt_id=receipt_id,
        import_id=import_id,
        staged_payload_path=str(staged),
        payload_sha256="b" * 64,
        payload_byte_length=4096,
    )
    artifact.validate()

    with pytest.raises(CodecSdkError, match="raw_import.source_path"):
        replace(request, source_path="relative.safetensors").validate()
    with pytest.raises(CodecSdkError, match="raw_import.payload_entry"):
        replace(raw_import_metadata(), payload_entry="../escape.safetensors").validate()
    with pytest.raises(CodecSdkError, match="raw_import.tensor_duplicate"):
        tensor = raw_import_metadata().tensors[0]
        replace(raw_import_metadata(), tensors=(tensor, tensor)).validate()
