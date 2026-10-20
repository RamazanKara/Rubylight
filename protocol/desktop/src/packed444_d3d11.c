#define COBJMACROS
#include <d3d11.h>
#include <d3dcompiler.h>
#include <stdint.h>
#include <stdlib.h>

typedef struct nj_packed {
    ID3D11DeviceContext *context, *deferred;
    ID3D11Texture2D *input;
    ID3D11ShaderResourceView *planes[2];
    ID3D11RenderTargetView *target;
    ID3D11VertexShader *vertex;
    ID3D11PixelShader *fragment;
    ID3D11Buffer *constants;
    int width, height;
} nj_packed;

#define RELEASE(p) do { if (p) { (p)->lpVtbl->Release(p); (p) = NULL; } } while (0)
#define CHECK(call) do { if (FAILED(call)) goto failed; } while (0)

void nj_packed_free(nj_packed *p) {
    if (!p) return;
    RELEASE(p->constants);
    RELEASE(p->vertex);
    RELEASE(p->fragment);
    RELEASE(p->target);
    RELEASE(p->planes[0]);
    RELEASE(p->planes[1]);
    RELEASE(p->input);
    RELEASE(p->deferred);
    RELEASE(p->context);
    free(p);
}

nj_packed *nj_packed_create(void *target, int width, int height, int ten_bit, const char *source, size_t length) {
    nj_packed *p = calloc(1, sizeof(*p));
    ID3D11Device *device = NULL;
    ID3DBlob *code = NULL, *errors = NULL;
    if (!p) return NULL;
    p->width = width;
    p->height = height;
    ID3D11Texture2D_GetDevice((ID3D11Texture2D *)target, &device);
    ID3D11Device_GetImmediateContext(device, &p->context);
    CHECK(ID3D11Device_CreateDeferredContext(device, 0, &p->deferred));
    D3D11_TEXTURE2D_DESC desc = {0};
    desc.Width = width;
    desc.Height = height;
    desc.MipLevels = desc.ArraySize = desc.SampleDesc.Count = 1;
    desc.Format = ten_bit ? DXGI_FORMAT_P010 : DXGI_FORMAT_NV12;
    desc.Usage = D3D11_USAGE_DEFAULT;
    desc.BindFlags = D3D11_BIND_SHADER_RESOURCE;
    CHECK(ID3D11Device_CreateTexture2D(device, &desc, NULL, &p->input));
    for (int i = 0; i < 2; ++i) {
        D3D11_SHADER_RESOURCE_VIEW_DESC view = {0};
        view.Format = ten_bit ? (i ? DXGI_FORMAT_R16G16_UNORM : DXGI_FORMAT_R16_UNORM)
                              : (i ? DXGI_FORMAT_R8G8_UNORM : DXGI_FORMAT_R8_UNORM);
        view.ViewDimension = D3D11_SRV_DIMENSION_TEXTURE2D;
        view.Texture2D.MipLevels = 1;
        CHECK(ID3D11Device_CreateShaderResourceView(device, (ID3D11Resource *)p->input, &view, &p->planes[i]));
    }
    CHECK(ID3D11Device_CreateRenderTargetView(device, (ID3D11Resource *)target, NULL, &p->target));
    CHECK(D3DCompile(source, length, NULL, NULL, NULL, "vertex", "vs_5_0", D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, &code, &errors));
    CHECK(ID3D11Device_CreateVertexShader(device, ID3D10Blob_GetBufferPointer(code), ID3D10Blob_GetBufferSize(code), NULL, &p->vertex));
    RELEASE(code);
    RELEASE(errors);
    CHECK(D3DCompile(source, length, NULL, NULL, NULL, "fragment", "ps_5_0", D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, &code, &errors));
    CHECK(ID3D11Device_CreatePixelShader(device, ID3D10Blob_GetBufferPointer(code), ID3D10Blob_GetBufferSize(code), NULL, &p->fragment));
    D3D11_BUFFER_DESC buffer = {0};
    buffer.ByteWidth = 48;
    buffer.Usage = D3D11_USAGE_DEFAULT;
    buffer.BindFlags = D3D11_BIND_CONSTANT_BUFFER;
    CHECK(ID3D11Device_CreateBuffer(device, &buffer, NULL, &p->constants));
    RELEASE(code);
    RELEASE(errors);
    RELEASE(device);
    return p;
failed:
    RELEASE(code);
    RELEASE(errors);
    RELEASE(device);
    nj_packed_free(p);
    return NULL;
}

/* The existing helper copies under the decoder device's lock. */
int nj_frame_copy_d3d11(const void *frame, void *destination, int width, int height);

int nj_packed_draw(nj_packed *p, const void *frame, const uint32_t parameters[12]) {
    if (nj_frame_copy_d3d11(frame, p->input, p->width, p->height) != 0) return 0;
    ID3D11DeviceContext *c = p->deferred;
    D3D11_VIEWPORT viewport = {0, 0, (float)parameters[0], (float)parameters[1], 0, 1};
    ID3D11DeviceContext_ClearState(c);
    ID3D11DeviceContext_UpdateSubresource(c, (ID3D11Resource *)p->constants, 0, NULL, parameters, 0, 0);
    ID3D11DeviceContext_OMSetRenderTargets(c, 1, &p->target, NULL);
    ID3D11DeviceContext_RSSetViewports(c, 1, &viewport);
    ID3D11DeviceContext_IASetPrimitiveTopology(c, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    ID3D11DeviceContext_VSSetShader(c, p->vertex, NULL, 0);
    ID3D11DeviceContext_PSSetShader(c, p->fragment, NULL, 0);
    ID3D11DeviceContext_PSSetShaderResources(c, 0, 2, p->planes);
    ID3D11DeviceContext_PSSetConstantBuffers(c, 0, 1, &p->constants);
    ID3D11DeviceContext_Draw(c, 3, 0);
    ID3D11CommandList *commands = NULL;
    if (FAILED(ID3D11DeviceContext_FinishCommandList(c, FALSE, &commands))) return 0;
    /* Restore SDL's cached context state after the reconstruction pass. */
    ID3D11DeviceContext_ExecuteCommandList(p->context, commands, TRUE);
    RELEASE(commands);
    return 1;
}
