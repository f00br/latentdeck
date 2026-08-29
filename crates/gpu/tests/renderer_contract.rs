use latentdeck_gpu::renderer::{
    Dx12Device, RgbaFrameRenderer, RgbaUpload, UploadError, create_dx12_instance,
    dx12_instance_descriptor,
};

#[test]
fn native_renderer_requests_only_dx12() {
    let descriptor = dx12_instance_descriptor();

    assert_eq!(descriptor.backends, wgpu::Backends::DX12);
}

#[test]
fn upload_contract_accepts_only_exact_abi_padded_rgba() {
    let padded = vec![0_u8; 512];
    let upload = RgbaUpload::new(3, 2, 256, &padded).expect("valid ABI upload");
    assert_eq!(upload.width(), 3);
    assert_eq!(upload.height(), 2);
    assert_eq!(upload.row_stride(), 256);
    assert_eq!(upload.bytes(), padded);

    assert!(matches!(
        RgbaUpload::new(3, 2, 12, &padded),
        Err(UploadError::InvalidRowStride { .. })
    ));
    assert!(matches!(
        RgbaUpload::new(3, 2, 256, &padded[..511]),
        Err(UploadError::InvalidDataLength { .. })
    ));
    assert!(matches!(
        RgbaUpload::new(0, 2, 256, &padded),
        Err(UploadError::InvalidLayout(_))
    ));
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn dx12_device_upload_and_fullscreen_triangle_submit() {
    let instance = create_dx12_instance().expect("DX12 backend is compiled on Windows");
    let context = Dx12Device::request(&instance, None)
        .await
        .expect("a DX12 adapter is required for LatentDeck");
    let renderer = RgbaFrameRenderer::new(context.device(), wgpu::TextureFormat::Rgba8Unorm, 3, 2)
        .expect("create renderer");
    let padded = vec![127_u8; 512];
    renderer
        .upload(
            context.queue(),
            RgbaUpload::new(3, 2, 256, &padded).expect("valid upload"),
        )
        .expect("upload frame");

    let target = context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("LatentDeck renderer contract target"),
        size: wgpu::Extent3d {
            width: 3,
            height: 2,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = context
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("LatentDeck renderer contract encoder"),
        });
    renderer.encode(&mut encoder, &view);
    context.queue().submit([encoder.finish()]);
    context
        .device()
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("DX12 submission completes");
}
