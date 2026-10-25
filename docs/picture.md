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
2. **In the compositor, for any window** (native, Proton, emulators, Android): the Picture profile. It
   sees only the finished picture, HUD included, with no depth and no motion vectors, so it does what
   needs neither: the curve, anti-aliasing (SMAA), scaling (FSR 1's upscale or a plain filter),
   sharpening.
3. **Frame generation:** OptiScaler's, where layer 1 works. Generating frames in the compositor without
   motion vectors (as Lossless Scaling does; `lsfg-vk` brings it to Linux as a Vulkan layer) is left open:
   it costs the GPU most when it has least to spare, and adds latency.

Not done: ReShade itself. Its compiler and presets would be a large C++ dependency, and its best-known
effects (ambient occlusion, depth of field) need the depth buffer, which only layer 1 sees. The few
effects worth having are written for our shaders instead.

## What the user sees

One tile in a game's settings, **Picture** (the HDR curve tile, renamed), opening the profile picker as
now: the game's own profile, Off, the named ones, New; the same menus. A profile holds everything below;
presets to start from cover the common cases (True black, Punchy, Gentle; Sharp: CAS only; Performance:
1280×720, FSR, SMAA; Pixel art: 960×540, Nearest, no anti-aliasing).

The edit screen has three pages, switched by tabs under the header (and the shoulder buttons):

- **Look:** the plot and the curve's rows, as now (black point, contrast, SDR white, highlights from,
  highlight peak, saturation, HDR game's peak).
- **Clarity:** four settings, each on its own, combined into one chain (below):
  - Resolution, asked of the game: Native (1920×1080 on the Thor), 1600×900, 1280×720, 960×540,
    640×360; on another screen the same fractions of it (⅚, ⅔, ½, ⅓).
  - Scaling, whenever the game's picture is smaller than the screen, whoever chose its size: Bilinear,
    Nearest, FSR. Nearest at ½ or ⅓ (960×540, 640×360 on the Thor) is exact integer scaling, which is
    why there is no integer setting; at the others it is uneven, and the row says so.
  - Anti-aliasing: Off, SMAA.
  - Sharpen: Off, then a strength. CAS, or FSR's own sharpening (RCAS) when Scaling is FSR, so it is
    never sharpened twice.
  Defaults: Native, Bilinear, Off, Off.
- **In game** (Proton games): Upscaler (the game's own, FSR 3.1, XeSS); Frame generation (Off, FSR,
  XeSS). Shown only once layer 1 is known to work on the device; until then not there.

Rows that do nothing for this game are dimmed with why, never hidden (the curve's peak for an HDR game;
Scaling with the picture at the screen's size; Resolution for a game that chooses its own; everything in
layer 2 under sway, where the compositor isn't ours). No combination is refused: SMAA before Nearest blurs
pixel edges, and the presets are where the sensible combinations are. One rule keeps the layers from
fighting: **with an upscaler in the game (In game's Upscaler, or the game's own temporal one), the
compositor's anti-aliasing and scaling are off** and dimmed ("the game upscales"); sharpening stays,
for those who want more.

The FPS tile and the HUD count frames shown, generated ones included; with frame generation on they say
so ("60 fps, 30 rendered"). The frame rate cap applies to frames shown.

## How it is done

**Layer 2, per window, a short chain of passes, each there only when switched on:**

1. SMAA at the game's own resolution: three passes (edges, blend weights, blending) with its two lookup
   textures. Fixed shaders, compiled once.
2. Scaling to the size the window is shown at: FSR 1's EASU (one pass), or the final pass sampling with
   Nearest or Bilinear. EASU works on the signal (gamma or PQ), so before decoding.
3. The final pass, into the scene: RCAS or CAS on what it reads, then the decoding, the curve and the
   conversion to the scene, as the input shader does now.

The anti-aliasing comes first so that the upscale doesn't enlarge jagged edges (AMD's own advice for
FSR 1); sharpening last, once. A stage switched off costs nothing: no pass, no texture. Intermediate
textures exist only for windows that need them, kept between frames, sized to the game's buffer.

**Shaders:** a variant is compiled for what changes the structure (the surface's colour description, and
whether the final pass sharpens); everything a slider moves is a uniform, so moving one never compiles.
Branches on uniforms are fine (every pixel takes the same one). Shader compiles stutter on the Adreno,
so variants are compiled when a window maps or a profile changes, never during play, and cached.

**Resolution:** the compositor already tells each window its size. A smaller resolution configures the
game's window smaller and shows it at the screen's size through the Scaling stage. A game that sets its
resolution itself (most Proton games, through Wine's display modes) ignores it; Scaling still applies to
whatever size its picture is.

**Layer 1:** ClamShell writes the game's `OptiScaler.ini` from its profile and starts it with
Proton-CachyOS's `PROTON_USE_OPTISCALER=1`, under the compositor and under sway alike. No files are put
into the game's folder.

**What replaces what:** `Setting::Upscale` (gamescope's FSR and integer modes, never implemented) goes;
Resolution and Scaling replace it. The curve's profiles become Picture profiles (a format change: the
saved curves are rewritten once by hand, as there are few).

**Licences:** FSR 1 (EASU, RCAS) and CAS are MIT; SMAA is MIT with its lookup textures; the filters are
ours.

## Phases

1. **On the desktop, nested and on the VM:** the tile renamed, the profile's body grown, the edit screen's
   pages; then CAS and RCAS in the final pass, Nearest and Bilinear, Resolution, EASU, SMAA, in that order. Each tested
   headless against reference images computed on the CPU (the shaders' arithmetic mirrored, as the
   curve's is), and by eye nested with a game.
2. **On the device (phase 4 of the compositor plan):** each stage's GPU time on the Adreno at 1080p and
   at ⅔; which presets are affordable. OptiScaler with a DX12 game through VKD3D on Turnip: does FSR
   3.1's upscaler run, at what cost, and frame generation, at what latency. The In game page follows only
   if it does.
3. **Later, if wanted:** a CRT shader for standalone emulators (RetroArch has its own); a LUT for colour
   grading; frame generation in the compositor, if measurements leave room.
