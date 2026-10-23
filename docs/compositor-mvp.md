# A compositor built into ClamShell: the MVP

Research report, 2026-10-23. Three passes: Smithay and the compositors built on it (niri, cosmic-comp,
Anvil), the Thor's display and input path (read from the kernel it runs: Armada with its patches), and how
the compositor fits into ClamShell's code. Nothing here has been tried on the device yet; section 5 says
what to try first.

## 1. Summary

- **It is feasible and well-trodden.** Smithay (Rust) runs daily-driver compositors (niri, COSMIC). An MVP
  for ClamShell is about **7,000-8,500 new lines and 2,900 deleted**, roughly 2.5-4 months of focused work
  to daily use, then a tail of per-app quirks (Steam, Proton).
- **The hardware favours it.** The Thor's kernel carries ROCKNIX's inline-rotation patch: a game's
  compressed (UBWC) buffer can be scanned out **rotated** onto the portrait-native panels, with no GPU
  composition. sway (wlroots) never does this, so today every game frame is composited. Smithay already
  attempts rotated scanout and falls back to composition when the hardware refuses.
- **It removes whole mechanisms, not just sway.** Synchronous placement deletes the landing workspace, the
  `for_window` rules with their switch variables, marks, stray-window matching and `pending_show`;
  the on-screen keyboard stops going through uinput; grim, sway's config and the generated output setup
  go; the nested-sway test becomes `cargo test`.
- **The main risks** are the DRM side on msm (two DSI panels, one in command mode; rotated scanout
  orientation; flip timestamps) and rewriting the state's window model. Both can be checked early: the
  first on the device in a day or two with niri as a stand-in, the second behind an adapter.

## 2. Scope (decided)

**In ClamShell's process**, on Smithay. Windows are state; a new window is placed before it is shown.
ClamShell exiting or restarting closes the session's apps: accepted. (The README's "a restart loses
nothing" stops being true when this lands.)

In the MVP:

- Outputs: both DSI panels through DRM/KMS, with their rotation; screen power; the top panel's refresh rate.
- Input: libinput (both touchscreens mapped to their panels, keyboards, pointers); the controller stays on
  evdev/uinput. libseat (logind).
- Clients: xdg-shell windows and popups, subsurfaces, dmabuf with feedback, viewporter, presentation-time,
  xdg-activation, data device and primary selection, pointer constraints and relative pointer (games),
  idle-inhibit. **Xwayland** (Steam, Proton).
- ClamShell's window model: on the top screen one window in front per group; on the second screen
  windows moved down, with tabs drawn by ClamShell.
- Drawing: direct scanout of a fullscreen game when the hardware accepts it, GPU composition otherwise.
  ClamShell's own screens stay CPU-drawn (tiny-skia), uploaded to textures by damage.
- Inside: the on-screen keyboard (text-input to apps, key events otherwise), idle, the clipboard,
  screenshots.
- A nested mode for development and a headless mode for end-to-end tests.

Later: FSR/upscaling, frame limiting beyond what the panel gives, VRR (the Thor's panels can't), HDR,
screen capture for other clients, layer-shell for third-party clients, multi-seat.

## 3. Architecture

### Where it lives

- A new crate `crates/compositor` (Smithay, server side) owns the event loop and depends on `ui` and
  `state`. `system` stays independent; the binary still connects effects to its threads.
- `ui` becomes a library around a backend-free core (state, input machines, timers, recording and drawing
  each screen into a pixmap). During the migration the current Wayland client hosts the same core; it is
  deleted at the end.
- One calloop loop on the main thread: the Wayland display, libinput, udev, DRM vblanks, Xwayland and the
  system threads' channel are sources on it. Smithay 0.7 and ClamShell both use calloop 0.14.
- **Smithay from git.** The last release (0.7.0) is from June 2025; main is active (hundreds of commits
  since, last read 2026-10-20) and has what we need that 0.7 lacks (the compositor-side text-input hooks).
  niri and cosmic-comp pin a git revision; so would we. A bump costs hours to a day of API churn.

### Windows as state

- The compositor reports `WindowEvent::Opened` (id, app id, title, the client's pid and its systemd unit,
  parent, Wayland or X11), `Closed`, `Title`, `Activate` (an xdg-activation request).
- Placement is synchronous: on a toplevel's first commit the compositor reads its pid and unit (a few
  /proc reads, once per window), applies `Opened`, reads where the state put it, and sends the first
  configure with the final size (1920x1080 top, 1240x1080 bottom, a dialog its own size). The window shows
  when its first buffer arrives.
- The state holds each window's place (`Top(group)` or `Bottom`), the front, and the window shown below.
  The compositor renders from that, as the views do: what each output shows, keyboard focus, configure
  sizes, visibility (for idle-inhibit and frame callbacks).
- Effects shrink to actions (`Close`); placement, focus and moves become state that the compositor reads.
  Screen power and refresh become one sync-driven `Outputs` effect.
- Settle loses the steps that only reconciled sway's view with ours (`show_pending`, `show_reopened`,
  `follow_closed`, `landing_up`, `windows_to_their_entries`, `stray_to_waiting`, `follow_window_rules`);
  starting entries found by app id and ending windowless ones become rules on `Opened` and `Closed`.
- `apply` runs inside Smithay's dispatch, so the compositor's own effects are queued and carried out after
  dispatch, as the UI already does for events.
- Title changes allocate (strings); the compositor coalesces them to at most one per frame.
- About 178 uses of today's window snapshot event in 21 test files, and the state-machines document's
  sections on launches, the front and the second screen, are rewritten.

### Drawing

- Each screen keeps one tiny-skia pixmap, always holding the latest frame, and a texture; on a drawn frame
  only the damaged rectangle is uploaded (`ImportMem::update_memory`). A custom render element reports
  that damage to Smithay's damage tracker, so composition is incremental too. (Smithay's own
  `MemoryRenderBuffer` allocates a list per frame and merges damage into one rectangle; avoid it.)
- Element order per output: the top shows the front window (or shelf's screen), then any ClamShell
  overlay, then the cursor; the bottom shows ClamShell's screen, or a window moved down with ClamShell's bar.
- Frames are paced by each output's vblank. Rasterising the UI happens on the compositor thread, so the top
  output's frame is queued first and the UI drawn after it in the same turn; moving playback to a worker is
  the fallback if measurements demand it.
- The cursor is ours to draw (an xcursor theme, about 150 lines).

### Input

- libinput events are hit-tested: ClamShell's own targets (the existing press machinery) or a client
  surface through Smithay's seat.
- Touchscreens are mapped by device name to their panel, with the output transform applied (libinput has
  no output mapping of its own).
- The on-screen keyboard: printable text goes to the focused surface's text-input as `commit_string` when
  it has one active; otherwise key events go through the seat (Xwayland and SDL games have no
  text-input). Activation of a text field drives the keyboard tab as today. The uinput keyboard goes.
- The virtual touchpad stays a uinput device for now: it exists to get libinput's touchpad behaviour, and
  our libinput sees it.

### Xwayland

Two ways, both used by real compositors:

- **xwayland-satellite** (niri since 2025-08): a separate process that turns X11 windows into ordinary
  xdg toplevels and popups. One window model, no X11 code in ClamShell; if it crashes, X11 apps go but the
  session stays. Needs only xdg-shell and viewporter. Open issues include Steam windows not reappearing,
  cut-off input regions, wine cursors.
- **Smithay's built-in X11 window manager** (COSMIC, Anvil): full X11 control in-process, at about 2,000
  lines of X11 handling plus X11 branches in focus and placement; long-open Smithay issues around Xwayland
  crashes and leaks; COSMIC still has open Steam/Proton issues.

**Chosen: Smithay's built-in X11 window manager, contained in one adapter.** Smithay's `desktop::Window`
already wraps a Wayland toplevel or an X11 surface, so the differences stay in one module of the
compositor, as one small `enum Surface { Wayland(..), X11(..) }` with a method per concern:

| Concern | Wayland | X11 |
|---|---|---|
| identity | `app_id` | `WM_CLASS` |
| the client's pid, for its scope | socket credentials | the XRes extension (`get_client_pid`) |
| placing and sizing | the first configure, before it maps | `configure_window`, answering its map request |
| fullscreen, dialog, parent | `set_fullscreen`, parent toplevel | `_NET_WM_STATE`, `WM_TRANSIENT_FOR` |
| menus and tooltips | `xdg_popup` with a positioner | override-redirect windows, made relative to their parent |
| focus | keyboard enter | `SetInputFocus`, `WM_TAKE_FOCUS` |
| closing | `close` | `WM_DELETE_WINDOW`, else kill |

The state sees one window type and the same events; placement is synchronous for both (an X11 map request
is held until the state has placed the window). Xwayland is a direct client, so dmabuf feedback (UBWC,
scanout, plane rotation) and the pacing protocols reach X11 games with no relay, and Wine's Wayland driver
is optional rather than required. Cost: about 1,500-2,000 lines in the adapter, and the X11 connection in
our process: Smithay has open issues around Xwayland crashing (a deadlock, a leaked process), so Xwayland
is a supervised child (its exit handled, restarted) and every X11 call is non-blocking, so a stuck
Xwayland can't stall the loop. Expect per-app quirks with Steam and Proton either way; these become ours
to fix in our own model.

What comes later works on surfaces, below the adapter, so it is shared by both kinds: frame-rate limiting
(the compositor paces frame callbacks and presentation feedback, which Xwayland's Present follows),
measuring frames (commit and presentation times per surface; this could replace MangoHud's log as
ClamShell's frame-rate source), and upscaling (the buffer drawn larger with FSR). Only asking a game to
render smaller differs, and is one more adapter method: a Wayland game is told its size per client
(configure and the outputs it sees), an X11 game picks a mode from Xwayland's RandR emulation, which
already hands the compositor a small buffer marked for scaling.

The alternative considered was xwayland-satellite (niri): X11 windows arrive as ordinary Wayland windows
from a separate process, at about 500 lines, but with its own translation losses (Steam issues open) and,
for us, the risk that per-surface dmabuf feedback and the pacing protocols don't pass through it, leaving
X11 games composited.

### Protocols

Smithay provides everything in the MVP list. It lacks wlr-screencopy, wlr output management/power and
tearing-control; none is needed, since screenshots, output settings and power are internal.

## 4. The Thor's display path

From the kernel the Thor runs (Armada at d2bcf6f with its patches) and Mesa:

- **One DRM device** (msm) drives the GPU and both panels. Pick the card by driver name: simpledrm can
  appear first under UEFI boot.
- **Top panel** (ICNA3520, DSI-2): **command mode** with DSC and two layer mixers, so every plane on it
  takes two hardware pipes. Modes 120 and 60 Hz; **switching between them is a full modeset with about
  0.4 s of black** (the panel is re-initialised). With no commit the panel refreshes from its own memory,
  so a 60 fps game on the 120 Hz mode is shown evenly with no extra traffic.
- **Bottom panel** (CH13726A, DSI-1): video mode, 60 Hz only.
- **Rotation:** both panels are portrait-native and report their orientation through the connector's
  panel-orientation property; read it rather than hard-coding.
- **Planes:** 10 pipes (4 that can scale, VIG; 6 that can't, DMA). Scanout buffers: linear or UBWC.
  **90°/270° plane rotation (Armada patch 0066, from ROCKNIX) works on VIG pipes, UBWC buffers only,**
  8888 and 2101010 formats, height before rotation up to 1088 (a 1920x1080 landscape game fits). A rotated
  fullscreen plane on the top panel takes two of the four VIG pipes.
- **Smithay sets the plane rotation from the element and output transforms and test-commits it,** falling
  back to composition when refused: rotated direct scanout needs no code of ours, only **dmabuf feedback
  with a scanout tranche**, so that Turnip and freedreno allocate UBWC (they pick it whenever offered).
- **The second way to scanout, already planned** (`docs/thor-gameplan.md`, "Direct scanout"): games render
  rotated themselves. The compositor tells a fullscreen surface the transform it prefers
  (`wl_surface.preferred_buffer_transform`; Smithay can send it, niri does), Mesa's Wayland WSI is patched
  to report it as `currentTransform` and set the buffer transform, and DXVK's present pass, which already
  copies into the swapchain image, writes it rotated (VKD3D-Proton likewise). Then the buffer matches the
  panel, scans out with no plane rotation, linear or UBWC, on a pipe that needn't scale, leaving the
  scaling pipes free. It needs Wine's Wayland driver: an X11 window under Xwayland can't set a buffer
  transform. **Order:** plane rotation first (no patches to Mesa or DXVK, so it covers native, Proton and
  emulated games alike); the pre-rotation patches only if measurements show a gap (games not getting UBWC,
  or pipes running short).
- **Timing:** no async (tearing) page flips; explicit sync supported. Armada patches 0078 and 0108 fix
  flip timestamps for command- and video-mode panels.
- **Power:** disabling a CRTC runs the panel's power-down. The bottom panel's driver lacks a guard against
  brightness writes while off: ClamShell must not write its backlight then.
- **Touch** reports each panel's native portrait coordinates; keep `61-thor-touch.hwdb`. The bottom
  touchscreen has its own supply (tap-to-wake with the panel off works).
- Kernel bug outside scope: plugging DisplayPort freezes the device (Armada #529).

## 5. Risks, and how to check them first

Before writing the backend, on the device (a day or two):

1. `drm_info` and `modetest`: connectors, modes, panel orientation, planes, formats and modifiers, rotation
   values, CRTC count.
2. **niri as a stand-in Smithay compositor** on the device: both panels with transforms, touch mapping,
   scanout logged (`RUST_LOG=smithay=trace`), vkcube or a game fullscreen with asymmetric content; Anvil
   (`ANVIL_DISABLE_DIRECT_SCANOUT` to compare) as a second check.
3. The risks below as scripted checks, then a power comparison: composited vs scanned out, 120 vs 60 Hz.

| Risk | Likelihood | Check |
|---|---|---|
| Rotated scanout wrong way round, mirrored or halves swapped (the patch and Smithay each flip transforms) | medium | asymmetric content fullscreen under niri; also 1280x720 scaled to fullscreen |
| Not enough scaling pipes (a rotated top game takes 2 of 4); then composition, costing power not correctness | medium | `drm_info`; plan for one scanout plane per screen |
| Flip timestamps early or wrong, feeding frame pacing (seen on msm by cosmic-comp, fixed in Smithay) | medium-high | flip-to-flip intervals at 120 Hz; render on the flip event, no predictive scheduling at first |
| A refresh change is a 0.4 s black modeset | certain | decide the policy (section 7) |
| A modeset or power change on one screen stalls the other | low-medium | toggle bottom power while the top animates; count missed flips, read dmesg |
| Brightness writes racing power or frames | medium | sweep brightness across power toggles; never write the bottom backlight while off |
| Games not getting UBWC (no scanout tranche, Xwayland, Proton) | medium | buffer modifiers while a game runs, native and through Proton; the pre-rotated way (section 4) doesn't need UBWC |
| Xwayland crashing or stalling inside our loop (Smithay's open issues) | medium | kill Xwayland while a game and Steam run: the compositor must carry on, restart it, and log |
| UBWC scanout corruption | low | vkcube fullscreen with scanout, by eye |
| Touch mapping or stuck contacts after resume | low | four-corner test; tolerate a contact that never lifts |
| Sleep and resume, libseat re-activation | low-medium | full commit on both outputs after resume; lid sleep with the compositor |
| UI rasterising delays game frames | low-medium | measure; move playback to a worker if needed |
| The state rewrite | high effort, low risk with tests | behind the adapter (section 6), with the state machines and tests rewritten together |

## 6. Phases (the product works at each step)

0. **Prepare under sway, no Smithay:** extract the UI core; route by screen; add the unified `Outputs`
   effect. Behaviour unchanged.
1. **Compositor crate, nested and headless,** hosting the core. A temporary adapter (about 300 lines)
   presents its windows as today's window snapshot and carries out today's window commands, so the state is
   untouched while the compositor matures. `clamshell --nested` on the desktop; the device stays on sway.
   Headless tests are written against what is visible from outside (which window on which output,
   configure sizes) so they survive phase 3.
2. **Device backend:** DRM/GBM/GLES, libseat, libinput, rotation, power, refresh, Xwayland, dmabuf
   feedback, the internal screenshot, the cursor, key injection and text-input. A session setting chooses
   sway or ClamShell; both stay installed while it is tried on the device.
3. **The state's window model with synchronous placement:** the adapter, the sway module, the choice, sway,
   grim, swaybg, the sway configs, the output-setup script and the nested-sway test go; the VM suite, the
   self-test and the panel calibration use ClamShell's own control words.
4. **Absorb, later:** the clipboard and the Waydroid proxy in-process, libinput replacing the evdev watch
   thread, notifications, uwsm, refresh calibration from vblank timestamps, direct injection of the pad
   mouse and keymap keys.

## 7. Decisions still open

| Decision | Recommendation |
|---|---|
| Xwayland: satellite or built-in X11 WM | decided: built-in, contained in one adapter (section 3) |
| Notifications without mako (no layer-shell): ClamShell's own low-battery warning reaches nothing otherwise | ClamShell draws its own notices on the top screen, and serves the notification D-Bus API itself later; minimal layer-shell (about 250 lines) only if third-party overlays turn out to matter |
| The top screen's tabs: sway shows a tab strip when a group has several windows | none on the top screen; switch a group's windows from the second screen; dialogs drawn over their parent |
| The top panel's refresh: a change blanks it for 0.4 s | stay at 120 Hz and limit a game's frame rate: in command mode the host sends only the frames committed, so a 60 fps game at the 120 Hz mode costs the host what it would at 60 Hz; what remains is the panel scanning its own memory at 120 Hz. Measure the battery at both modes with the same 60 fps game and a static screen; if the difference is small, drop the 60 Hz mode; if it matters, look for a seamless switch of the panel's internal rate (a vendor command, a kernel driver change) rather than a blanking modeset |
| Nested mode: one host window with both screens stacked, or two windows | one window, both screens stacked (Smithay's winit backend gives one) |
| Smithay: release 0.7 or a git revision | a pinned git revision |
| Allocation in the compositor's hot path | ours zero, enforced; Smithay's per-frame allocations counted and pinned in a reference so they can't grow unnoticed |
| `unsafe` (Smithay's renderer and EGL constructors are unsafe) | one small module allowed it, with SAFETY comments |

## 8. Testing

- `crates/compositor/tests` under `cargo test`: a headless backend with Smithay's software renderer
  (pixman: no GPU, deterministic), two outputs at the Thor's sizes, test clients in the same loop over a
  socketpair, stepped until a condition holds (no sleeps), pid-to-unit lookup behind a trait. niri's
  `src/tests/` (about 5,000 lines) is the model.
- Ported from the nested-sway test, and new: a launch's window in front; a second window of a launch; a
  slow app not shown early; down and up with rules placing at the first configure; quit; the front falling
  back; a stray window; dialogs over their parent; text-input commit and key injection; the clipboard; touch
  routing; screenshot pixel hashes; the allocation budget. Xwayland tests opt-in.
- A control socket with replies (`ctl windows`, `ctl screenshot FILE`) for the VM suite and any remaining
  shell test.
- The VM suite keeps DRM on virtio-gpu, libinput, logind and the session's files (GLES through llvmpipe, or
  pixman on DRM if that fails); the device covers rotation, both panels, refresh, power, scanout and frame
  pacing.

## 9. Size

| Part | Lines | Effort |
|---|---|---|
| UI core extraction (phase 0) | ~600 changed | low |
| Event loop, Smithay state and protocol handlers | ~1,200 | medium |
| UI texture element with damage | ~250 | low |
| Input routing, touch mapping, focus, cursor | ~750 | medium |
| DRM backend: two rotated outputs, power, mode change, vblank pacing, scanout | ~1,200 | high (the device risk) |
| Nested and headless backends | ~550 | low-medium |
| Xwayland (the built-in X11 window manager, in one adapter) | ~1,500-2,000 | medium-high (app quirks) |
| Placement, configure sizes, dialogs, close | ~500 | medium |
| Text input and key injection | ~250 | medium |
| Screenshot, idle-inhibit, clipboard, session | ~400 | low |
| The state's window model | ~1,000 code, ~2,000 test lines changed | high (the logic risk) |
| Packages, calibration, self-test, VM suite | ~300 | low |
| **New** | **~7,000-8,500** | |
| **Deleted in the MVP** | **~2,900** (sway module, the Wayland client host, the uinput keyboard, grim, the nested-sway test, configs, restart adoption, settle's window steps) | |

For comparison: niri reached its first alpha in about 3.5 months at 12.9k lines, written from scratch;
Anvil, Smithay's reference compositor, is about 10k. ClamShell already has its state, UI and system
layers, which is why its MVP is smaller.

## Sources

Smithay (github.com/Smithay/smithay, read at 24647fc, 2026-10-20; docs.rs for 0.7.0), niri (e19df12c;
`src/backend/tty.rs`, `headless.rs`, `src/tests/`, `docs/wiki`), cosmic-comp, xwayland-satellite (v0.8.3)
and their open issues; the Armada kernel and patches (0066 inline rotation from ROCKNIX, 0076, 0078, 0107,
0108), `drivers/gpu/drm/msm/disp/dpu1`, the Thor's device tree, the panel drivers; Mesa (Turnip
`tu_image.cc`, freedreno `freedreno_resource.c`); cosmic-comp #1062 and Smithay PR #1685 (msm timestamps);
Armada #28 (both Thor panels driven by KWin with gamescope), #276, #529; ClamShell's own code as of
2026-10-23. The three research reports went into this document; their full file and line references are
kept in this session's record.
