/* The test pattern of crates/compositor/examples/pattern-game.rs (`pixel`, the same here), for the C test
 * games: tests/sdl-pattern.c and tests/wine-pattern.c. */

#ifndef PATTERN_H
#define PATTERN_H

#include <math.h>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

static void grey(unsigned char *c, int v) { c[0] = c[1] = c[2] = (unsigned char)v; }

static void set(unsigned char *c, int r, int g, int b) { c[0] = (unsigned char)r; c[1] = (unsigned char)g; c[2] = (unsigned char)b; }

static int corner(int x, int y, int w, int h, int right, int bottom) {
    return (right ? x >= w - 16 : x < 16) && (bottom ? y >= h - 16 : y < 16);
}

/* The pattern's colour at pixel (x, y) of w by h, as red, green, blue. */
static void pixel(int x, int y, int w, int h, unsigned char *c) {
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

#endif
