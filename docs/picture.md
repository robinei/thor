# Picture: curves, anti-aliasing, upscaling, sharpening, frame generation

How a game's picture is changed on its way to the screen, as one design: what the user sees and sets,
and how the compositor and the launch do it. The tone curve (`docs/compositor-mvp.md`, phase 2h) is the
first part of it and is done; the rest follows the order in "Phases" below.

## Three layers

1. **In the game, through OptiScaler** (Proton games with an upscaler of their own: DLSS 2+, FSR 2+,
   XeSS). OptiScaler is a DLL loaded next to the game (`WINEDLLOVERRIDES`; Proton-CachyOS 11 does it with
   `PROTON_USE_OPTISCALER=1`, copying nothing into the game's folder). It takes the game's upscaler calls
   and runs another upscaler, and can add frame generation (FSR's or XeSS's), with the game's motion
   vectors and depth, before its HUD. It is the best quality there is, where it works. **Whether it works
   on the Adreno is unknown**: FSR 4 needs RDNA 3 or 4; FSR 3.1's upscaler and frame generation are DX12
   compute through VKD3D, which Turnip can run in principle, but no one reports it, and VKD3D is the
   Adreno 740's weak spot. So it is a test on the device first (Phases).
2. **In the compositor, for any window** (native, Proton, emulators, Android): the colour profile and the
   clarity settings. It sees only the finished picture, HUD included, with no depth and no motion vectors, so it does what
   needs neither: the curve, anti-aliasing (SMAA), scaling (FSR 1's upscale or a plain filter),
   sharpening.
3. **Frame generation:** OptiScaler's, where layer 1 works. Generating frames in the compositor without
   motion vectors (as Lossless Scaling does; `lsfg-vk` brings it to Linux as a Vulkan layer) is left open:
   it costs the GPU most when it has least to spare, and adds latency.

Not done: ReShade itself. Its compiler and presets would be a large C++ dependency, and its best-known
effects (ambient occlusion, depth of field) need the depth buffer, which only layer 1 sees. The few
effects worth having are written for our shaders instead.

## What the user sees

Every setting is a tile in the settings grid (five columns, four rows), as the others are: a tap steps to
the next value, a long press opens the tile's menu (below). No screens in between.

- **Colour** (the tone curve tile): the curve profile, as now: a game's own, Default colours, the named ones. Tapping
  steps through them; the menu is the picker, with the edit screen (the plot and the curve's rows: black
  point, contrast, SDR white, highlights from, highlight peak, saturation, HDR game's peak). A LUT for
  colour grading, if it comes, is part of this profile.
- **Clarity**, four ordinary settings, each a tile, with the All / runner / game levels every setting
  has (Nearest set once on a pixel-art emulator's runner, 1280×720 on one heavy game, the rest inherited):
  - Resolution, the screen as the game sees it: Native (1920×1080 on the Thor), 1600×900, 1280×720,
    960×540, 640×360; on another screen the same fractions of it (⅚, ⅔, ½, ⅓). The tile shows the size.
  - Scaling, whenever the game's picture is smaller than the screen, whoever chose its size: Bilinear,
    Nearest+Integer, FSR (a CRT look, if it comes, is one more). Nearest+Integer shows the picture at
    the largest whole multiple of its buffer that fits, centred, so every pixel is a square of whole
    screen pixels; nearest at any other factor makes pixels of uneven width, which shimmer. ½ and ⅓
    (960×540, 640×360 on the Thor) fill the screen; a game's own 640×480 shows at 2× between bars. A
    picture of another shape than its window (a mode stretched over the screen) is fitted in its own
    shape with Bilinear and FSR too.
  - Anti-aliasing: Off, FXAA, SMAA. FXAA is one pass and softer (it smooths fine detail and text along
    with edges); SMAA finds edges by their patterns and leaves detail, in three passes.
  - Sharpen: Off, 25, 50, 75, 100 %. CAS, or FSR's own sharpening (RCAS) when Scaling is FSR, so it is
    never sharpened twice.
  Defaults: Native, Bilinear, Off, Off.
- **In game** (Proton games), two more tiles, once layer 1 is known to work on the device: Upscaler (the
  game's own, FSR 3.1, XeSS) and Frame generation (Off, FSR, XeSS).

**A tile's menu** (a long press, or holding A): every value of the setting, to pick one directly, and
Reset to the level below ("Reset to All's: Native") when this level overrides it. A profile tile's
menu is its picker (with New, and each profile's Edit, Rename, Duplicate, Delete), with the same Reset.
Reset for a whole level stays its own tile.

Tiles that do nothing for this game are dimmed with why, never hidden (Scaling with the picture at the
screen's size; everything in layer 2 under sway, where the compositor isn't ours); so are rows of the
curve's edit screen (the curve's peak for an HDR game). No combination is refused: SMAA before Nearest
blurs pixel edges, and the levels are where the sensible combinations are kept. One rule keeps the layers
from fighting: **with an upscaler in the game (In game's Upscaler, or the game's own temporal one), the
compositor's anti-aliasing and scaling are off** and dimmed ("the game upscales"); sharpening stays,
for those who want more.

The FPS tile and the HUD count frames shown, generated ones included; with frame generation on they say
so ("60 fps, 30 rendered"). The frame rate cap applies to frames shown.

## How it is done

**Layer 2, per window: SMAA when on, then one final pass, with one pass more only for FSR with
sharpening.**

1. SMAA at the game's own resolution: three passes (edges, blend weights, blending) with its two lookup
   textures. Fixed shaders, compiled once.
2. The final pass, into the scene at the size the window is shown at: it samples the game's picture
   (Nearest or Bilinear, as the texture is sampled; FSR 1's EASU, which reads the source around each
   pixel; CAS in its scaling mode, which sharpens and scales at once), then decodes, applies the curve
   and converts to the scene, as the input shader does now. EASU works on the signal (gamma or PQ), as
   it is before decoding.
3. Only with FSR and sharpening: RCAS reads the upscaled pixels around each pixel, so EASU writes to a
   texture first and the final pass sharpens what it reads from it.

The anti-aliasing comes first so that the upscale doesn't enlarge jagged edges (AMD's own advice for
FSR 1); sharpening once.

Passes before the final one, by what is set, each the fewest it can be:

| Set | Passes before the final one |
|---|---|
| Sharpening (CAS), Nearest, Bilinear, FSR unsharpened | none: all in the final pass |
| FSR shown one to one | none: it upscales nothing, so it is sampled as it is and sharpened by CAS |
| FXAA or SMAA's blending, shown one to one or Nearest, unsharpened | FXAA none; SMAA two (edges, weights), its blending in the final pass at the texel's centre |
| FXAA with Bilinear, FSR or sharpening after it | one, at the game's resolution |
| SMAA with Bilinear, FSR or sharpening after it | three, at the game's resolution |
| FSR with sharpening | one more, EASU at the screen's resolution; RCAS in the final pass |

Fusing further costs more than it saves: SMAA's blending or FXAA inside EASU would be done for each of
EASU's 12 reads at the screen's resolution, where now it is done once per game pixel; EASU inside RCAS
would be done five times per pixel, so AMD's own FSR 1 is two passes. Before scaling or sharpening the
anti-aliased picture is read around each pixel, so it has to be in a texture. Textures between passes
are bytes for SDR, half floats for HDR; SMAA's blending in the final pass binds its weights to a second
texture unit around Smithay's draw (see Smithay below), and the window is then drawn whole each time
its buffer changes, as the blending reaches past what the client damaged. A stage switched off costs nothing: no pass, no texture. Intermediate textures
exist only for windows that need them, kept between frames, sized to the game's buffer.

**Shaders:** a variant is compiled for what changes the structure (the surface's colour description, how
the final pass samples, and whether it sharpens); everything a slider moves is a uniform, so moving one never compiles.
Branches on uniforms are fine (every pixel takes the same one). Shader compiles stutter on the Adreno,
so variants are compiled when a window maps or a profile changes, never during play, and cached.

**Resolution:** the game sees the chosen resolution as its screen. A client is known by its process
when it connects (its scope, or Steam's `SteamAppId` for a process Steam started), and a game's client
is told, when it binds the top output, that resolution as the output's current mode, so a game that
picks from the display modes (most Proton games, through Wine's Wayland driver) finds it as the largest
and fills it in fullscreen. It is told once: a change applies the next time the game starts, since Wine
lays its monitors out by their modes and a live change moves them under the game's window. The shared
mode is the screen's and never changes. A game sees only the top output (Wine would make the second
screen a monitor the game may open on), unless it shows a window on the second screen by a rule, as a
dual-screen emulator does. A window fixed at a size a resolution gives the screen fills it, as Wine
shows a game. The game may still choose less itself: Wine and Xwayland then send a smaller buffer scaled
by a viewport, and Scaling and the passes take it as they take any picture smaller than shown.
X11 games through Xwayland, one client for all of them, see the screen's size.

**Layer 1:** ClamShell writes the game's `OptiScaler.ini` from its settings and starts it with
Proton-CachyOS's `PROTON_USE_OPTISCALER=1`, under the compositor and under sway alike. No files are put
into the game's folder.

**What replaces what:** `Setting::Upscale` (gamescope's FSR and integer modes, never implemented) goes;
Resolution and Scaling replace it. The curve profiles stay as they are, as the colour.

**Licences:** FSR 1 (EASU, RCAS) and CAS are MIT; SMAA is MIT with its lookup textures; the filters are
ours.

## Smithay

Where Smithay (rev 24647fc) is in the way, worth contributing upstream:

- Texture programs take one texture. Fusing a pass that reads a second texture (SMAA's weights) needs
  raw GL around Smithay's draw: the texture bound to unit 1 before, unbound after, its sampler uniform
  set to unit 1 on every draw of the program (left at unit 0 beside an external texture it is a GL
  error). Upstream: textures as uniforms of custom programs (`UniformValue::Texture`), bound to units
  after the first by the renderer.
- Texture programs are GLSL ES 1.00 (`#version 100`, `texture2D`, no `textureLod` or offsets, no
  integer maths). SMAA needs ES 3.00, so its passes are raw GL in their own framebuffer with their own
  external-texture variant. Upstream: custom programs with a chosen version, the renderer's external and
  debug variants made for it too.
- A frame can't draw into a texture other than its target, so each pass is a frame of its own on its
  texture. Not a cost in itself; noted for when passes are batched.

## Phases

1. **On the desktop, nested and on the VM:** the four clarity tiles on a 5×4 grid (done); the tiles'
   menus, with profile tiles stepping through their profiles on a tap (done); then CAS, Nearest and
   Bilinear in the final pass (done), Resolution (done: vkcube at ½ with Nearest measured as exact 2× in
   the VM), EASU (done, in the final pass), RCAS (done: with Sharpen, EASU goes into a texture kept per
   window, `colour::chain`, and the final pass sharpens it), SMAA (done: the reference implementation's
   three passes, vendored in `colour/smaa/`, as ES 3 programs in Smithay's context, first in the chain;
   they read the game's buffer as it is, an ordinary texture or an external one, copying it only when its
   client turned it; SMAA's picture is in bytes for SDR and half floats for HDR; in the VM it nearly triples
   the share of in-between tones across vkcube's edges), in that order. A window whose passes fail is shown
   without them, logged once. Not yet tried: an external buffer through SMAA (none in the VM). SMAA's blending
   in the final pass (done) when shown one to one or Nearest and unsharpened: in the VM vkcube's edges
   have 56 to 59 % in-between tones, 49 % at 960×540 Nearest counted on game pixels. FXAA
   (done): in the final pass when the window is shown one to one and not sharpened after it, else one pass
   in the chain at the game's resolution; in the VM it gives 81 % in-between tones across vkcube's
   edges, SMAA 49 %, none 24 %. Each tested headless against
   reference images computed on the CPU (the shaders' arithmetic mirrored, as the curve's is), and by
   eye nested with a game.
2. **On the device (phase 4 of the compositor plan):** each stage's GPU time on the Adreno at 1080p and
   at ⅔; which combinations are affordable. OptiScaler with a DX12 game through VKD3D on Turnip: does FSR
   3.1's upscaler run, at what cost, and frame generation, at what latency. The In game tiles follow only
   if it does.
3. **Later, if wanted:** a CRT shader for standalone emulators (RetroArch has its own); a LUT for colour
   grading; frame generation in the compositor, if measurements leave room.
