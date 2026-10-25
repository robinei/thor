/* A game through SDL 3, on Wayland or X11 (SDL_VIDEO_DRIVER), drawing the test pattern of
 * crates/compositor/examples/pattern-game.rs (`pixel`, the same here) at the size it renders at. It
 * prints the fullscreen modes it is offered (`mode W H`, the desktop's first as `desktop W H`) and reads
 * commands on stdin: `mode W H` switches to that exclusive fullscreen mode (as a game picking a lower
 * resolution), `desktop` to fullscreen at the desktop's size, `quit` ends it. It prints `size W H`, the
 * size it draws at, whenever that changes. `sdl-pattern --ppm W H` writes the pattern as an image to
 * stdout instead.
 *
 * Built by tests/picture-scaling.py: cc tests/sdl-pattern.c $(pkg-config --cflags --libs sdl3) -lm. */

#include <SDL3/SDL.h>
#include <math.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static void grey(Uint8 *c, int v) { c[0] = c[1] = c[2] = (Uint8)v; }

static void set(Uint8 *c, int r, int g, int b) { c[0] = (Uint8)r; c[1] = (Uint8)g; c[2] = (Uint8)b; }

static int corner(int x, int y, int w, int h, int right, int bottom) {
    return (right ? x >= w - 16 : x < 16) && (bottom ? y >= h - 16 : y < 16);
}

/* The pattern's colour at pixel (x, y) of w by h, as red, green, blue. */
static void pixel(int x, int y, int w, int h, Uint8 *c) {
    int cx = w / 2, cy = h / 2;
    if (corner(x, y, w, h, 0, 0)) set(c, 255, 0, 0);
    else if (corner(x, y, w, h, 1, 0)) set(c, 0, 255, 0);
    else if (corner(x, y, w, h, 0, 1)) set(c, 0, 0, 255);
    else if (corner(x, y, w, h, 1, 1)) set(c, 255, 255, 0);
    else if (x == 0 || y == 0 || x == w - 1 || y == h - 1) grey(c, 255);
    else if (x == cx || y == cy) set(c, 255, 0, 255);
    else if (x < cx && y < cy) {
        static const double angles[4] = {5.0, 20.0, 45.0, 70.0};
        int band = x * 4 / (cx > 1 ? cx : 1);
        double angle = angles[band < 0 ? 0 : band > 3 ? 3 : band] * M_PI / 180.0;
        double along = x * sin(angle) - y * cos(angle);
        double cell = floor(along / 6.0);
        grey(c, fmod(cell, 2.0) == 0.0 ? 255 : 0);
    } else if (y < cy) {
        double dx = x - (cx + w) / 2, dy = y - cy / 2;
        int r = (w - cx) < cy ? (w - cx) : cy;
        if (hypot(dx, dy) > r * 0.45) grey(c, 128);
        else grey(c, (long long)floor((atan2(dy, dx) / (2 * M_PI) + 1.0) * 72.0) % 2 == 0 ? 255 : 0);
    } else if (x < cx) {
        int band = (y - cy) * 4 / ((h - cy) > 1 ? (h - cy) : 1);
        int on = band <= 0 ? x % 2 == 0 : band == 1 ? x % 4 < 2 : band == 2 ? y % 2 == 0 : (x + y) % 2 == 0;
        grey(c, on ? 255 : 0);
    } else {
        int u = x - cx, v = y - cy, bw = (w - cx) > 1 ? w - cx : 1, bh = (h - cy) > 1 ? h - cy : 1;
        if (v < bh / 2) grey(c, u * 255 / bw);
        else if (u < bw / 2) grey(c, u >= bw / 4 ? 255 : 0);
        else {
            double t = (u - bw * 3 / 4) / 16.0 + 0.5;
            t = t < 0 ? 0 : t > 1 ? 1 : t;
            grey(c, (int)lround(t * t * (3.0 - 2.0 * t) * 255.0));
        }
    }
}

/* The pattern at w by h into a streaming texture, as XRGB8888. */
static SDL_Texture *pattern(SDL_Renderer *renderer, int w, int h) {
    SDL_Texture *texture = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_XRGB8888, SDL_TEXTUREACCESS_STREAMING, w, h);
    void *pixels;
    int pitch;
    if (!texture || !SDL_LockTexture(texture, NULL, &pixels, &pitch)) return texture;
    SDL_SetTextureScaleMode(texture, SDL_SCALEMODE_NEAREST);
    for (int y = 0; y < h; y++) {
        Uint32 *row = (Uint32 *)((Uint8 *)pixels + (size_t)y * pitch);
        for (int x = 0; x < w; x++) {
            Uint8 c[3];
            pixel(x, y, w, h, c);
            row[x] = 0xff000000u | (Uint32)c[0] << 16 | (Uint32)c[1] << 8 | c[2];
        }
    }
    SDL_UnlockTexture(texture);
    return texture;
}

static int ppm(int w, int h) {
    printf("P6\n%d %d\n255\n", w, h);
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) {
            Uint8 c[3];
            pixel(x, y, w, h, c);
            fwrite(c, 1, 3, stdout);
        }
    return 0;
}

int main(int argc, char **argv) {
    int pw, ph;
    if (argc == 4 && strcmp(argv[1], "--ppm") == 0 && sscanf(argv[2], "%d", &pw) == 1 && sscanf(argv[3], "%d", &ph) == 1) return ppm(pw, ph);

    if (!SDL_Init(SDL_INIT_VIDEO)) {
        fprintf(stderr, "SDL_Init: %s\n", SDL_GetError());
        return 1;
    }
    SDL_DisplayID display = SDL_GetPrimaryDisplay();
    const SDL_DisplayMode *desktop = SDL_GetDesktopDisplayMode(display);
    if (!desktop) {
        fprintf(stderr, "no desktop mode: %s\n", SDL_GetError());
        return 1;
    }
    printf("desktop %d %d\n", desktop->w, desktop->h);
    int count = 0;
    SDL_DisplayMode **modes = SDL_GetFullscreenDisplayModes(display, &count);
    for (int i = 0; modes && i < count; i++) printf("mode %d %d\n", modes[i]->w, modes[i]->h);
    SDL_Window *window = SDL_CreateWindow("SDL pattern", desktop->w, desktop->h, SDL_WINDOW_FULLSCREEN);
    SDL_Renderer *renderer = window ? SDL_CreateRenderer(window, NULL) : NULL;
    if (!renderer) {
        fprintf(stderr, "window: %s\n", SDL_GetError());
        return 1;
    }
    SDL_SetRenderVSync(renderer, 1);
    fflush(stdout);

    char line[128];
    size_t used = 0;
    int last_w = 0, last_h = 0;
    SDL_Texture *texture = NULL;
    for (;;) {
        SDL_Event event;
        while (SDL_PollEvent(&event))
            if (event.type == SDL_EVENT_QUIT) return 0;
        struct pollfd in = {STDIN_FILENO, POLLIN, 0};
        if (poll(&in, 1, 0) > 0) {
            ssize_t got = read(STDIN_FILENO, line + used, sizeof line - used - 1);
            if (got <= 0) return 0;
            used += (size_t)got;
            line[used] = 0;
            char *end;
            while ((end = strchr(line, '\n'))) {
                *end = 0;
                int w, h;
                if (sscanf(line, "mode %d %d", &w, &h) == 2) {
                    SDL_DisplayMode mode;
                    if (SDL_GetClosestFullscreenDisplayMode(display, w, h, 0.0f, false, &mode)) {
                        SDL_SetWindowFullscreenMode(window, &mode);
                        SDL_SetWindowFullscreen(window, true);
                    } else {
                        fprintf(stderr, "no mode %dx%d: %s\n", w, h, SDL_GetError());
                    }
                } else if (strcmp(line, "desktop") == 0) {
                    SDL_SetWindowFullscreenMode(window, NULL);
                    SDL_SetWindowFullscreen(window, true);
                } else if (strcmp(line, "quit") == 0) {
                    return 0;
                }
                SDL_SyncWindow(window);
                last_w = 0;
                memmove(line, end + 1, strlen(end + 1) + 1);
                used = strlen(line);
            }
        }
        int w, h;
        SDL_GetCurrentRenderOutputSize(renderer, &w, &h);
        if (w != last_w || h != last_h) {
            printf("size %d %d\n", w, h);
            fflush(stdout);
            last_w = w;
            last_h = h;
            if (texture) SDL_DestroyTexture(texture);
            texture = pattern(renderer, w, h);
        }
        SDL_RenderTexture(renderer, texture, NULL, NULL);
        SDL_RenderPresent(renderer);
    }
}
