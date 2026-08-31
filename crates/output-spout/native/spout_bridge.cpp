#include "spout_bridge.h"

#include <array>
#include <cstring>
#include <memory>
#include <new>

#include <d3d11_4.h>
#include <d3d12.h>
#include <SpoutDX12.h>

namespace {

constexpr uint32_t kStatusSchema = 1;
constexpr uint32_t kMaxDimension = 16384;
constexpr size_t kMaxRequestedNameBytes = 240;
constexpr size_t kMaxWrappedResources = 8;
constexpr uint32_t kDxgiFormatRgba8Unorm = 28;
constexpr uint32_t kDxgiFormatBgra8Unorm = 87;
constexpr uint32_t kShaderResourceState = static_cast<uint32_t>(
    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE |
    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);

struct wrapped_resource {
    ID3D12Resource* source = nullptr;
    ID3D11Resource* wrapped = nullptr;
    uint32_t initial_state = 0;
};

bool valid_name(const char* name) noexcept {
    if (name == nullptr) {
        return false;
    }
    const size_t length = strnlen_s(name, 256);
    if (length == 0 || length > kMaxRequestedNameBytes) {
        return false;
    }
    if (name[0] == ' ' || name[length - 1] == ' ') {
        return false;
    }
    for (size_t index = 0; index < length; ++index) {
        const unsigned char character = static_cast<unsigned char>(name[index]);
        if (character < 0x20 || character > 0x7e) {
            return false;
        }
    }
    return true;
}

bool valid_format(uint32_t format) noexcept {
    return format == kDxgiFormatRgba8Unorm || format == kDxgiFormatBgra8Unorm;
}

bool valid_initial_state(uint32_t state) noexcept {
    return state == D3D12_RESOURCE_STATE_COMMON ||
           state == D3D12_RESOURCE_STATE_RENDER_TARGET ||
           state == D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE ||
           state == kShaderResourceState ||
           state == D3D12_RESOURCE_STATE_COPY_DEST ||
           state == D3D12_RESOURCE_STATE_COPY_SOURCE;
}

}  // namespace

struct latentdeck_spout_sender {
    spoutDX12 spout;
    ID3D12Device* device = nullptr;
    ID3D12CommandQueue* queue = nullptr;
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t format = 0;
    bool opened = false;
    std::array<char, 256> requested_name{};
    std::array<wrapped_resource, kMaxWrappedResources> resources{};

    ~latentdeck_spout_sender() noexcept {
        for (wrapped_resource& resource : resources) {
            if (resource.wrapped != nullptr) {
                resource.wrapped->Release();
                resource.wrapped = nullptr;
            }
            if (resource.source != nullptr) {
                resource.source->Release();
                resource.source = nullptr;
            }
        }
        if (opened) {
            spout.ReleaseSender();
            spout.CloseDirectX12();
            opened = false;
        }
        if (queue != nullptr) {
            queue->Release();
            queue = nullptr;
        }
        device = nullptr;  // The matching AddRef is owned/released by spoutDX12.
    }
};

namespace {

void fill_status(latentdeck_spout_sender* sender, latentdeck_spout_status* status) noexcept {
    if (status == nullptr) {
        return;
    }
    *status = {};
    status->schema = kStatusSchema;
    if (sender == nullptr) {
        return;
    }
    status->published = sender->spout.IsInitialized() ? 1U : 0U;
    status->width = sender->width;
    status->height = sender->height;
    status->format = sender->format;
    status->spout_frame = status->published != 0 ? sender->spout.GetFrame() : 0;
    const char* active_name = sender->spout.GetName();
    if (active_name == nullptr || active_name[0] == '\0') {
        active_name = sender->requested_name.data();
    }
    strncpy_s(status->active_name, active_name, _TRUNCATE);
}

int32_t set_name(latentdeck_spout_sender* sender, const char* sender_name) noexcept {
    if (sender == nullptr || !valid_name(sender_name)) {
        return LATENTDECK_SPOUT_INVALID_ARGUMENT;
    }
    if (sender->spout.IsInitialized()) {
        sender->spout.ReleaseSender();
    }
    strncpy_s(sender->requested_name.data(), sender->requested_name.size(), sender_name, _TRUNCATE);
    sender->spout.SetSenderName(sender->requested_name.data());
    sender->spout.SetSenderFormat(static_cast<DXGI_FORMAT>(sender->format));
    return LATENTDECK_SPOUT_OK;
}

bool same_com_identity(IUnknown* left, IUnknown* right) noexcept {
    if (left == nullptr || right == nullptr) {
        return false;
    }
    IUnknown* left_identity = nullptr;
    IUnknown* right_identity = nullptr;
    const HRESULT left_result =
        left->QueryInterface(IID_PPV_ARGS(&left_identity));
    const HRESULT right_result =
        right->QueryInterface(IID_PPV_ARGS(&right_identity));
    const bool matches = SUCCEEDED(left_result) && SUCCEEDED(right_result) &&
                         left_identity != nullptr &&
                         left_identity == right_identity;
    if (left_identity != nullptr) {
        left_identity->Release();
    }
    if (right_identity != nullptr) {
        right_identity->Release();
    }
    return matches;
}

bool resource_belongs_to_device(
    latentdeck_spout_sender* sender,
    ID3D12Resource* resource) noexcept {
    ID3D12Device* resource_device = nullptr;
    const HRESULT result = resource->GetDevice(IID_PPV_ARGS(&resource_device));
    if (FAILED(result) || resource_device == nullptr) {
        return false;
    }
    const bool matches = same_com_identity(resource_device, sender->device);
    resource_device->Release();
    return matches;
}

bool queue_belongs_to_device(ID3D12CommandQueue* queue, ID3D12Device* device) noexcept {
    ID3D12Device* queue_device = nullptr;
    const HRESULT result = queue->GetDevice(IID_PPV_ARGS(&queue_device));
    if (FAILED(result) || queue_device == nullptr) {
        return false;
    }
    const bool matches = same_com_identity(queue_device, device);
    queue_device->Release();
    return matches;
}

bool enable_multithread_protection(latentdeck_spout_sender* sender) noexcept {
    ID3D11DeviceContext* immediate_context = sender->spout.GetD3D11context();
    if (immediate_context == nullptr) {
        return false;
    }
    ID3D11Multithread* multithread = nullptr;
    const HRESULT result =
        immediate_context->QueryInterface(IID_PPV_ARGS(&multithread));
    if (FAILED(result) || multithread == nullptr) {
        return false;
    }
    multithread->SetMultithreadProtected(TRUE);
    multithread->Release();
    return true;
}

bool compatible_resource(latentdeck_spout_sender* sender, ID3D12Resource* resource) noexcept {
    const D3D12_RESOURCE_DESC description = resource->GetDesc();
    return description.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D &&
           description.Width == sender->width && description.Height == sender->height &&
           static_cast<uint32_t>(description.Format) == sender->format &&
           description.DepthOrArraySize == 1 && description.MipLevels == 1 &&
           description.SampleDesc.Count == 1 && description.SampleDesc.Quality == 0;
}

wrapped_resource* find_wrapped(
    latentdeck_spout_sender* sender,
    ID3D12Resource* resource) noexcept {
    for (wrapped_resource& candidate : sender->resources) {
        if (candidate.source == resource) {
            return &candidate;
        }
    }
    return nullptr;
}

wrapped_resource* find_empty(latentdeck_spout_sender* sender) noexcept {
    for (wrapped_resource& candidate : sender->resources) {
        if (candidate.source == nullptr) {
            return &candidate;
        }
    }
    return nullptr;
}

bool wrap_resource_preserving_state(
    latentdeck_spout_sender* sender,
    ID3D12Resource* resource,
    uint32_t state,
    ID3D11Resource** out_wrapped) noexcept {
    ID3D11On12Device* d3d11_on_12 = sender->spout.GetD3D11On12device();
    if (d3d11_on_12 == nullptr || out_wrapped == nullptr) {
        return false;
    }
    D3D11_RESOURCE_FLAGS flags = {};
    if (state == D3D12_RESOURCE_STATE_RENDER_TARGET) {
        flags.BindFlags = D3D11_BIND_RENDER_TARGET;
    }
    const auto exact_state = static_cast<D3D12_RESOURCE_STATES>(state);
    const HRESULT result = d3d11_on_12->CreateWrappedResource(
        resource,
        &flags,
        exact_state,
        exact_state,
        IID_PPV_ARGS(out_wrapped));
    return SUCCEEDED(result) && *out_wrapped != nullptr;
}

}  // namespace

extern "C" int32_t latentdeck_spout_sender_open(
    void* d3d12_device,
    void* direct_command_queue,
    const char* sender_name,
    uint32_t width,
    uint32_t height,
    uint32_t dxgi_format,
    latentdeck_spout_sender** out_sender,
    latentdeck_spout_status* out_status) noexcept {
    if (out_sender == nullptr || out_status == nullptr) {
        return LATENTDECK_SPOUT_INVALID_ARGUMENT;
    }
    *out_sender = nullptr;
    *out_status = {};
    try {
        if (d3d12_device == nullptr || direct_command_queue == nullptr ||
            !valid_name(sender_name) || width == 0 || height == 0 ||
            width > kMaxDimension || height > kMaxDimension ||
            !valid_format(dxgi_format)) {
            return LATENTDECK_SPOUT_INVALID_ARGUMENT;
        }

        auto* device = static_cast<ID3D12Device*>(d3d12_device);
        auto* queue = static_cast<ID3D12CommandQueue*>(direct_command_queue);
        if (queue->GetDesc().Type != D3D12_COMMAND_LIST_TYPE_DIRECT ||
            !queue_belongs_to_device(queue, device)) {
            return LATENTDECK_SPOUT_DEVICE_MISMATCH;
        }

        std::unique_ptr<latentdeck_spout_sender> sender(
            new (std::nothrow) latentdeck_spout_sender());
        if (!sender) {
            return LATENTDECK_SPOUT_INTERNAL_ERROR;
        }
        sender->device = device;
        sender->queue = queue;
        sender->width = width;
        sender->height = height;
        sender->format = dxgi_format;
        sender->queue->AddRef();

        // SpoutDX12 releases the supplied external device but does not AddRef it.
        // Give it an explicit owned reference before calling the official API.
        sender->device->AddRef();
        IUnknown* queue_unknown = sender->queue;
        if (!sender->spout.OpenDirectX12(sender->device, &queue_unknown) ||
            sender->spout.GetD3D11On12device() == nullptr) {
            return LATENTDECK_SPOUT_OPEN_FAILED;
        }
        sender->opened = true;
        if (!enable_multithread_protection(sender.get())) {
            return LATENTDECK_SPOUT_OPEN_FAILED;
        }

        const int32_t name_result = set_name(sender.get(), sender_name);
        if (name_result != LATENTDECK_SPOUT_OK) {
            return name_result;
        }

        fill_status(sender.get(), out_status);
        *out_sender = sender.release();
        return LATENTDECK_SPOUT_OK;
    }
    catch (...) {
        *out_sender = nullptr;
        *out_status = {};
        return LATENTDECK_SPOUT_INTERNAL_ERROR;
    }
}

extern "C" int32_t latentdeck_spout_sender_set_name(
    latentdeck_spout_sender* sender,
    const char* sender_name,
    latentdeck_spout_status* out_status) noexcept {
    try {
        if (out_status == nullptr) {
            return LATENTDECK_SPOUT_INVALID_ARGUMENT;
        }
        const int32_t result = set_name(sender, sender_name);
        fill_status(sender, out_status);
        return result;
    }
    catch (...) {
        if (out_status != nullptr) {
            *out_status = {};
        }
        return LATENTDECK_SPOUT_INTERNAL_ERROR;
    }
}

extern "C" int32_t latentdeck_spout_sender_release(
    latentdeck_spout_sender* sender,
    latentdeck_spout_status* out_status) noexcept {
    try {
        if (sender == nullptr || out_status == nullptr) {
            return LATENTDECK_SPOUT_INVALID_ARGUMENT;
        }
        sender->spout.ReleaseSender();
        fill_status(sender, out_status);
        return LATENTDECK_SPOUT_OK;
    }
    catch (...) {
        if (out_status != nullptr) {
            *out_status = {};
        }
        return LATENTDECK_SPOUT_INTERNAL_ERROR;
    }
}

extern "C" int32_t latentdeck_spout_sender_send_resource(
    latentdeck_spout_sender* sender,
    void* d3d12_resource,
    uint32_t initial_resource_state,
    latentdeck_spout_status* out_status) noexcept {
    try {
        if (sender == nullptr || d3d12_resource == nullptr || out_status == nullptr) {
            return LATENTDECK_SPOUT_INVALID_ARGUMENT;
        }
        if (!valid_initial_state(initial_resource_state)) {
            return LATENTDECK_SPOUT_INVALID_ARGUMENT;
        }
        auto* resource = static_cast<ID3D12Resource*>(d3d12_resource);
        if (!resource_belongs_to_device(sender, resource)) {
            return LATENTDECK_SPOUT_DEVICE_MISMATCH;
        }
        if (!compatible_resource(sender, resource)) {
            return LATENTDECK_SPOUT_RESOURCE_MISMATCH;
        }

        wrapped_resource* wrapped = find_wrapped(sender, resource);
        if (wrapped != nullptr && wrapped->initial_state != initial_resource_state) {
            return LATENTDECK_SPOUT_RESOURCE_MISMATCH;
        }
        if (wrapped == nullptr) {
            wrapped = find_empty(sender);
            if (wrapped == nullptr) {
                return LATENTDECK_SPOUT_RESOURCE_LIMIT;
            }
            ID3D11Resource* wrapped_resource_11 = nullptr;
            // The official SpoutDX12 helper hardcodes OutState=PRESENT. LatentDeck
            // reuses a wgpu-owned texture whose tracker expects the exact state
            // supplied by the caller (combined pixel/non-pixel shader resource
            // for pinned wgpu 30),
            // so use the official D3D11On12 device with identical In/Out states.
            // The subsequent send remains the official SendDX11Resource path.
            if (!wrap_resource_preserving_state(
                    sender, resource, initial_resource_state, &wrapped_resource_11)) {
                return LATENTDECK_SPOUT_WRAP_FAILED;
            }
            resource->AddRef();
            wrapped->source = resource;
            wrapped->wrapped = wrapped_resource_11;
            wrapped->initial_state = initial_resource_state;
        }

        if (!sender->spout.SendDX11Resource(wrapped->wrapped)) {
            return LATENTDECK_SPOUT_SEND_FAILED;
        }
        fill_status(sender, out_status);
        if (out_status->published == 0) {
            return LATENTDECK_SPOUT_SEND_FAILED;
        }
        return LATENTDECK_SPOUT_OK;
    }
    catch (...) {
        if (out_status != nullptr) {
            *out_status = {};
        }
        return LATENTDECK_SPOUT_INTERNAL_ERROR;
    }
}

extern "C" void latentdeck_spout_sender_destroy(latentdeck_spout_sender* sender) noexcept {
    try {
        delete sender;
    }
    catch (...) {
        // Destruction crosses a C ABI and cannot report a failure. All owned
        // resources use noexcept COM Release paths; this guard is defensive.
    }
}
