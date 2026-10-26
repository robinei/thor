/* A game as a Windows program under Wine with DXVK: a borderless fullscreen window drawing the test
 * pattern (pattern.h) through a Direct3D 11 swapchain, SDR, or with `--hdr` HDR10's: 10 bits a channel,
 * PQ with BT.2020's primaries, its white at 1000 nits (DXVK takes it to Wayland's colour management). It
 * prints the screen as Windows has it (`desktop W H`) and the modes offered (`mode W H`), then `size W H`
 * whenever it draws at a new size; it reads commands on stdin: `mode W H`
 * switches the display to that mode as a game does (ChangeDisplaySettings, which Wine emulates) and draws
 * at it, `desktop` goes back, `quit` ends it.
 *
 * Built by tests/picture-scaling.py:
 *   zig cc -target x86_64-windows-gnu tests/wine-pattern.c -o wine-pattern.exe -ld3d11 -ldxgi */

#define COBJMACROS
#include <windows.h>
#include <d3d11.h>
#include <dxgi1_6.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "pattern.h"

#define WM_MODE (WM_APP + 1)
#define WM_DESKTOP (WM_APP + 2)

static HWND window;
static int hdr;
static int desktop_w, desktop_h;

/* sRGB's decoding of a pattern channel, as light 0 to 1. */
static double light(unsigned char c) {
    double e = c / 255.0;
    return e <= 0.04045 ? e / 12.92 : pow((e + 0.055) / 1.055, 2.4);
}

/* PQ's encoding of light in nits, 0 to 1. */
static double pq(double nits) {
    double p = pow(nits / 10000.0, 2610.0 / 16384.0);
    return pow((3424.0 / 4096.0 + 2413.0 / 128.0 * p) / (1.0 + 2392.0 / 128.0 * p), 2523.0 / 32.0);
}

/* The pattern at w by h as the swapchain's pixels: RGBA8, or for HDR R10G10B10A2 with PQ's encoding. */
static unsigned int *pattern(int w, int h) {
    unsigned int *pixels = malloc((size_t)w * h * 4);
    if (!pixels) return NULL;
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) {
            unsigned char c[3];
            pixel(x, y, w, h, c);
            unsigned int v;
            if (hdr) {
                unsigned int r = (unsigned int)lround(pq(light(c[0]) * 1000.0) * 1023.0);
                unsigned int g = (unsigned int)lround(pq(light(c[1]) * 1000.0) * 1023.0);
                unsigned int b = (unsigned int)lround(pq(light(c[2]) * 1000.0) * 1023.0);
                v = r | g << 10 | b << 20 | 3u << 30;
            } else {
                v = c[0] | (unsigned int)c[1] << 8 | (unsigned int)c[2] << 16 | 0xffu << 24;
            }
            pixels[(size_t)y * w + x] = v;
        }
    return pixels;
}

/* A command, posted to the window; false for quit. */
static int command(const char *line) {
    int w, h;
    if (sscanf(line, "mode %d %d", &w, &h) == 2) PostMessageW(window, WM_MODE, (WPARAM)w, (LPARAM)h);
    else if (strncmp(line, "desktop", 7) == 0) PostMessageW(window, WM_DESKTOP, 0, 0);
    else if (strncmp(line, "quit", 4) == 0) return 0;
    return 1;
}

/* The commands, from stdin. */
static DWORD WINAPI commands(void *unused) {
    (void)unused;
    char line[128];
    while (fgets(line, sizeof line, stdin))
        if (!command(line)) break;
    PostMessageW(window, WM_CLOSE, 0, 0);
    return 0;
}

static LRESULT CALLBACK procedure(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam) {
    switch (message) {
    case WM_MODE: {
        DEVMODEW mode = {.dmSize = sizeof mode, .dmFields = DM_PELSWIDTH | DM_PELSHEIGHT};
        mode.dmPelsWidth = (DWORD)wparam;
        mode.dmPelsHeight = (DWORD)lparam;
        LONG result = ChangeDisplaySettingsExW(NULL, &mode, NULL, CDS_FULLSCREEN, NULL);
        if (result != DISP_CHANGE_SUCCESSFUL) fprintf(stderr, "ChangeDisplaySettings %dx%d: %ld\n", (int)wparam, (int)lparam, result);
        SetWindowPos(hwnd, HWND_TOP, 0, 0, (int)wparam, (int)lparam, SWP_SHOWWINDOW);
        return 0;
    }
    case WM_DESKTOP:
        ChangeDisplaySettingsExW(NULL, NULL, NULL, 0, NULL);
        SetWindowPos(hwnd, HWND_TOP, 0, 0, desktop_w, desktop_h, SWP_SHOWWINDOW);
        return 0;
    case WM_DESTROY:
        PostQuitMessage(0);
        return 0;
    }
    return DefWindowProcW(hwnd, message, wparam, lparam);
}

int main(int argc, char **argv) {
    hdr = argc > 1 && strcmp(argv[1], "--hdr") == 0;
    setvbuf(stdout, NULL, _IONBF, 0);
    DEVMODEW now = {.dmSize = sizeof now};
    EnumDisplaySettingsW(NULL, ENUM_CURRENT_SETTINGS, &now);
    desktop_w = (int)now.dmPelsWidth;
    desktop_h = (int)now.dmPelsHeight;
    printf("desktop %d %d\n", desktop_w, desktop_h);
    DWORD last_w = 0, last_h = 0;
    DEVMODEW m = {.dmSize = sizeof m};
    for (DWORD i = 0; EnumDisplaySettingsW(NULL, i, &m); i++) {
        if (m.dmBitsPerPel != 32 || (m.dmPelsWidth == last_w && m.dmPelsHeight == last_h)) continue;
        printf("mode %lu %lu\n", m.dmPelsWidth, m.dmPelsHeight);
        last_w = m.dmPelsWidth;
        last_h = m.dmPelsHeight;
    }

    WNDCLASSW class = {.lpfnWndProc = procedure, .hInstance = GetModuleHandleW(NULL), .lpszClassName = L"pattern", .hCursor = LoadCursorW(NULL, (LPCWSTR)IDC_ARROW)};
    RegisterClassW(&class);
    window = CreateWindowExW(WS_EX_APPWINDOW, L"pattern", L"Wine pattern", WS_POPUP | WS_VISIBLE, 0, 0, desktop_w, desktop_h, NULL, NULL, class.hInstance, NULL);
    if (!window) {
        fprintf(stderr, "no window\n");
        return 1;
    }

    ID3D11Device *device;
    ID3D11DeviceContext *context;
    if (FAILED(D3D11CreateDevice(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, 0, NULL, 0, D3D11_SDK_VERSION, &device, NULL, &context))) {
        fprintf(stderr, "no Direct3D 11 device (is DXVK in the prefix?)\n");
        return 1;
    }
    IDXGIFactory2 *factory;
    if (FAILED(CreateDXGIFactory1(&IID_IDXGIFactory2, (void **)&factory))) {
        fprintf(stderr, "no DXGI factory\n");
        return 1;
    }
    DXGI_FORMAT format = hdr ? DXGI_FORMAT_R10G10B10A2_UNORM : DXGI_FORMAT_R8G8B8A8_UNORM;
    DXGI_SWAP_CHAIN_DESC1 desc = {.Format = format, .SampleDesc = {1, 0}, .BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT, .BufferCount = 2, .SwapEffect = DXGI_SWAP_EFFECT_FLIP_DISCARD};
    IDXGISwapChain1 *swapchain;
    if (FAILED(IDXGIFactory2_CreateSwapChainForHwnd(factory, (IUnknown *)device, window, &desc, NULL, NULL, &swapchain))) {
        fprintf(stderr, "no swapchain\n");
        return 1;
    }
    if (hdr) {
        IDXGISwapChain3 *three;
        if (FAILED(IDXGISwapChain1_QueryInterface(swapchain, &IID_IDXGISwapChain3, (void **)&three)) ||
            FAILED(IDXGISwapChain3_SetColorSpace1(three, DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020))) {
            fprintf(stderr, "no HDR10 colour space\n");
            return 1;
        }
        IDXGISwapChain3_Release(three);
    }
    CreateThread(NULL, 0, commands, NULL, 0, NULL);

    int w = 0, h = 0;
    unsigned int *pixels = NULL;
    for (;;) {
        MSG message;
        while (PeekMessageW(&message, NULL, 0, 0, PM_REMOVE)) {
            if (message.message == WM_QUIT) return 0;
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        RECT client;
        GetClientRect(window, &client);
        int cw = client.right - client.left, ch = client.bottom - client.top;
        if (cw <= 0 || ch <= 0) {
            Sleep(10);
            continue;
        }
        if (cw != w || ch != h) {
            if (FAILED(IDXGISwapChain1_ResizeBuffers(swapchain, 0, (UINT)cw, (UINT)ch, DXGI_FORMAT_UNKNOWN, 0))) {
                fprintf(stderr, "ResizeBuffers %dx%d failed\n", cw, ch);
                return 1;
            }
            free(pixels);
            pixels = pattern(cw, ch);
            w = cw;
            h = ch;
            printf("size %d %d\n", w, h);
        }
        ID3D11Texture2D *back;
        if (SUCCEEDED(IDXGISwapChain1_GetBuffer(swapchain, 0, &IID_ID3D11Texture2D, (void **)&back))) {
            if (pixels) ID3D11DeviceContext_UpdateSubresource(context, (ID3D11Resource *)back, 0, NULL, pixels, (UINT)w * 4, 0);
            ID3D11Texture2D_Release(back);
        }
        IDXGISwapChain1_Present(swapchain, 1, 0);
    }
}
