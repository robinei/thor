# Picture: curves, anti-aliasing, upscaling, sharpening, frame generation

How a game's picture is changed on its way to the screen, as one design: what the user sees and sets,
and how the compositor and the launch do it. The HDR curve (`docs/compositor-mvp.md`, phase 2h) is the
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

- **Colour** (the HDR curve tile): the curve profile, as now: a game's own, Off, the named ones. Tapping
  steps through them; the menu is the picker, with the edit screen (the plot and the curve's rows: black
  point, contrast, SDR white, highlights from, highlight peak, saturation, HDR game's peak). A LUT for
  colour grading, if it comes, is part of this profile.
- **Clarity**, four ordinary settings, each a tile, with the All / runner / game levels every setting
  has (Nearest set once on a pixel-art emulator's runner, 1280×720 on one heavy game, the rest inherited):
  - Resolution, the screen as the game sees it: Native (1920×1080 on the Thor), 1600×900, 1280×720,
    960×540, 640×360; on another screen the same fractions of it (⅚, ⅔, ½, ⅓). The tile shows the size.
  - Scaling, whenever the game's picture is smaller than the screen, whoever chose its size: Bilinear,
    Nearest, FSR (a CRT look, if it comes, is one more). Nearest at ½ or ⅓ (960×540, 640×360 on the
    Thor) is exact integer scaling, which is why there is no integer setting; at the others it is
    uneven, and the tile says so.
  - Anti-aliasing: Off, SMAA.
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
FSR 1); sharpening once. A stage switched off costs nothing: no pass, no texture. Intermediate textures
exist only for windows that need them, kept between frames, sized to the game's buffer.

**Shaders:** a variant is compiled for what changes the structure (the surface's colour description, how
the final pass samples, and whether it sharpens); everything a slider moves is a uniform, so moving one never compiles.
Branches on uniforms are fine (every pixel takes the same one). Shader compiles stutter on the Adreno,
so variants are compiled when a window maps or a profile changes, never during play, and cached.

**Resolution:** the game sees the chosen resolution as its screen. While it runs, the compositor reports
its screen's mode as that size (as gamescope gives a game a smaller screen), so a game that picks from the
display modes (most Proton games, through Wine) finds it as the largest and fills it in fullscreen, and
its window is configured to it. The game may still choose less; Scaling shows whatever size its picture
is at the screen's. The mode is the screen's for every client on it (Xwayland's included) while the game
runs; since the compositor tells every window its size, other windows hardly notice. Outputs per game
would avoid even that, if it ever matters.

**Layer 1:** ClamShell writes the game's `OptiScaler.ini` from its settings and starts it with
Proton-CachyOS's `PROTON_USE_OPTISCALER=1`, under the compositor and under sway alike. No files are put
into the game's folder.

**What replaces what:** `Setting::Upscale` (gamescope's FSR and integer modes, never implemented) goes;
Resolution and Scaling replace it. The curve profiles stay as they are, as the colour.

**Licences:** FSR 1 (EASU, RCAS) and CAS are MIT; SMAA is MIT with its lookup textures; the filters are
ours.

## Phases

1. **On the desktop, nested and on the VM:** the four clarity tiles on a 5×4 grid (done); the tiles'
   menus, with profile tiles stepping through their profiles on a tap (done); then CAS, Nearest and
   Bilinear in the final pass (done; FSR samples bilinear until EASU), Resolution (done: vkcube at ½
   with Nearest measured as exact 2× in the VM), EASU and RCAS, SMAA, in that order. Each tested headless against
   reference images computed on the CPU (the shaders' arithmetic mirrored, as the curve's is), and by
   eye nested with a game.
2. **On the device (phase 4 of the compositor plan):** each stage's GPU time on the Adreno at 1080p and
   at ⅔; which combinations are affordable. OptiScaler with a DX12 game through VKD3D on Turnip: does FSR
   3.1's upscaler run, at what cost, and frame generation, at what latency. The In game tiles follow only
   if it does.
3. **Later, if wanted:** a CRT shader for standalone emulators (RetroArch has its own); a LUT for colour
   grading; frame generation in the compositor, if measurements leave room.
