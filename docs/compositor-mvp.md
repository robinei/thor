# A compositor built into ClamShell: the MVP

Research report, 2026-10-23. Three passes: Smithay and the compositors built on it (niri, cosmic-comp,
Anvil), the Thor's display and input path (read from the kernel it runs: Armada with its patches), and how
the compositor fits into ClamShell's code. Nothing here has been tried on the device yet; section 5 says
what to try first.

## 1. Summary

- **It is feasible and well-trodden.** Smithay (Rust) runs daily-driver compositors (niri, COSMIC). An MVP
  for ClamShell is now estimated at **9,000-11,000 new production lines and 2,900 deleted**, roughly 3-5 months of focused work
  to daily use, then a tail of per-app quirks (Steam, Proton).
- **The hardware favours it.** The Thor's kernel carries ROCKNIX's inline-rotation patch: a game's
  compressed (UBWC) buffer can be scanned out **rotated** onto the portrait-native panels, with no GPU
  composition. sway (wlroots) never does this, so today every game frame is composited. Smithay already
  attempts rotated scanout and falls back to composition when the hardware refuses.
- **It removes whole mechanisms, not just sway.** Synchronous placement deletes the landing workspace, the
  `for_window` rules with their switch variables, marks and backend placement acknowledgements;
  the on-screen keyboard stops going through uinput; grim, sway's config and the generated output setup
  go; the nested-sway test becomes `cargo test`.
- **The main risks** are the DRM side on msm (two DSI panels, one in command mode; rotated scanout
  orientation; flip timestamps) and rewriting the state's window model. Both can be checked early: the
  first on the device in a day or two with niri as a stand-in, the second behind an adapter.

## 2. Scope (decided)

**In ClamShell's process**, on Smithay. Windows are state; a new window is placed before it is shown.
ClamShell exiting or restarting closes the session's apps: accepted. (The README's "a restart loses
nothing" stops being true when this lands. It also means updating ClamShell ends the session: a
`pacman -Syu` must not restart it while a game runs. The new version is installed and takes over at the
next login or an explicit restart of the session, which the maintenance playbook says.)

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
  Screen power and refresh use the sync-driven output effects introduced in phase 0.
- State assigns ownership, home groups, bottom rules and selection during apply. Foreground launch intent,
  replacement history, finding entries by app id and ending windowless entries remain domain rules.
  Landing workspaces, marks and retrying asynchronous placement belong only to the sway adapter.
- `apply` runs inside Smithay's dispatch, so the compositor's own effects are queued and carried out after
  dispatch, as the UI already does for events.
- Title changes allocate (strings); the compositor coalesces them to at most one per frame.
- Phase 1 replaces the window snapshot event in the state and its tests with lifecycle changes and
  updates the state-machines document's launch, front and second-screen contracts.

### Windows that aren't fullscreen

**MVP: one window at a time.** Each screen shows exactly one toplevel (with its popups): fullscreen, or
centred at its own size on black. The compositor reports a window's parent, size limits and type with
`Opened` and `Changed`; the state picks the presentation, and the compositor computes the geometry and maps
input through it. No window floats or is moved by hand.

- **Most windows** accept any size and are fullscreen.
- **A dialog** (a parent, `xdg_dialog_v1`, or X11 `WM_TRANSIENT_FOR` or a dialog window type) becomes the
  front of its parent's group, centred at its own size on black, its parent not shown; closing it returns
  to the parent. One larger than the screen is told the screen's size. Modal or not makes no difference: nothing beneath can be touched.
- **Windows that can't change size** (minimum and maximum equal, or an X11 splash or utility window) are
  centred at their own size, sharp at 1:1; one too big for the screen is scaled down to fit.
- **Windows that ignore their size** (mostly games rendering a fixed windowed resolution) are scaled to fit,
  keeping their aspect, with black bars (on the device the scaling pipes can do it without the GPU). The
  default follows the entry: fit for games, centred for apps.
- A touch on black around a window does nothing.

**Windows on the two screens** (the model, the same under sway and the compositor; no focus or mode):

1. **Every window lives on one screen** and stays there until moved, whichever program is active.
2. **At most one window is below, across all programs.** The second screen shows it, or ClamShell's own view
   instead (Home + right stick press).
3. **Moving needs no focus:** Home + right stick down moves the window shown on top down; if another was
   below, it goes back up into its program's windows on top. So a window of the same program is swapped
   (it is that program's next window on top, so it shows), and one of another program waits there until
   that program is activated. Home + right stick up brings the window below up and shows it. With nothing
   to move, the flick does nothing. A remembered window rule moving a new window down works the same way.
4. **Activating a program** (shelf, the dock, a launch) brings its most recent window on top to the front;
   the window below stays (shown, if it is that program's and ClamShell's view covered it).
5. **While Home is held, arrows at the top screen's edges** appear when the program shown there has another
   window on top; touching one, or Home + right stick left / right, switches to it. Nothing else is shown.
   The second screen needs none: it has one window.
6. **When the window shown on top closes**, the top shows that program's next window, else what was in
   front before, eventually shelf. When the window below closes, the second screen shows ClamShell's view.

The dock and shelf switch programs; the arrows switch a program's windows on top; placement is the user's.
Keyboard focus follows what was touched last, and pad input returns it to the top screen. A new dialog
becomes the front of its screen (it usually needs an answer); the arrows go back to its parent. The dock's
menu for an entry still lists its windows, to pick one by touch. Two windows below at once (a video and a
chat) is given up, and can be relaxed later if a need appears.

Under sway the model is the same (the state decides, the sway adapter carries it out with down, back up
and focus a tab). Sway keeps its own tab bar, so it needs no arrow overlay. Two other presentation
differences are accepted for a fallback:

- dialogs and fixed-size windows float over their parent, as sway does, instead of alone on black;
- a game that ignores its size is shown at that size, not scaled to fit.

**The Home chords.** Home is on the left of the Thor, Select too; Start, Back and the right stick are on
the right; the AYN button is in the middle. Every chord is Home under the left thumb with the right thumb or
an index finger, never two buttons under one thumb:

| Chord | Action |
|---|---|
| Home | shelf |
| Home + right stick down / up | the top screen's window down (the one below goes back up) / the window below up (was Home+Down/Up) |
| Home + right stick left / right | previous / next window of the program shown on the top screen |
| Home + right stick press | the second screen: ClamShell's view or the windows moved there (was Home+Select) |
| Home+B | close the window in front; held, quit the program |
| Home+R1 | screenshot |
| Home+Start | the second screen on or off |

A flick acts once: past about 70% in one direction (the axis further out), re-armed below about 30%; a
stick already off-centre when Home goes down must come back first. While Home is held the right stick goes
to ClamShell, and the game is sent a centred stick. The AYN button, reachable alone, is the candidate for
opening a Home menu on the second screen later. These chords and the model above don't depend on the
compositor: under sway, switching windows switches tabs.

**Later, considered:** dialogs whose parent should stay visible (find and replace, tool palettes, live
previews) lose context in the MVP; switching between a dialog and its parent from the second screen covers
it meanwhile. Two ways were weighed:

- Dialogs on the second screen, the parent untouched on top: no app hints needed, but the second screen is
  also the on-screen keyboard's and the touchpad's, and dialogs that take text would have to share it with
  the keyboard. Rejected.
- Dialogs overlaid on their parent, moved out of the way by touch: the likely direction, with its own
  questions (what can be moved, where it stays, what a tap on the parent does, modal vs non-modal from the
  apps' hints). For after the compositor has proven itself.

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

**Chosen: an in-process asynchronous X11 window manager, contained in one adapter.** The pinned
Smithay XWM makes blocking X11 round trips in event dispatch and Wayland commit hooks. A worker alone
cannot isolate those hooks, so it is not used. The adapter uses x11rb's protocol types and sequence
tracking with its own bounded non-blocking socket I/O; no X11 reply is waited for in the main loop.
This is more code than Smithay's XWM, accepted to keep a stopped Xwayland from stopping the session.
Wayland and X11 surfaces share presentation and input below the adapter, with one method per concern:

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
is optional rather than required. Revised cost: about 2,500-3,500 production lines in the adapter,
including its asynchronous request handling, socket activation, supervision and clipboard bridge, and the X11 connection in
our process: Smithay has open issues around Xwayland crashing (a deadlock, a leaked process), so Xwayland
is a supervised child (its exit handled, restarted) and every X11 call is non-blocking, so a stuck
Xwayland can't stall the loop. Expect per-app quirks with Steam and Proton either way; these become ours
to fix in our own model.

What comes later works on surfaces, below the adapter, so it is shared by both kinds: frame-rate limiting
(the compositor paces frame callbacks and presentation feedback, which Xwayland's Present follows),
measuring frames (commit and presentation times per surface, "Frame statistics and limiting" below),
and upscaling (the buffer drawn larger with FSR). Only asking a game to
render smaller differs, and is one more adapter method: a Wayland game is told its size per client
(configure and the outputs it sees), an X11 game picks a mode from Xwayland's RandR emulation, which
already hands the compositor a small buffer marked for scaling.

The alternative considered was xwayland-satellite (niri): X11 windows arrive as ordinary Wayland windows
from a separate process, at about 500 lines, but with its own translation losses (Steam issues open) and,
for us, the risk that per-surface dmabuf feedback and the pacing protocols don't pass through it, leaving
X11 games composited.

### X11/Wayland clipboard bridge

Both directions are implemented for both seat selections, with bounded asynchronous pipe I/O,
INCR handshakes, metadata and MIME/text targets, cancellation and real-client tests. XFixes watches
ownership without polling. Incoming offers carry a generation tag so they cannot echo into X11;
a conversion gets a fresh hidden requestor window so a late reply cannot feed its replacement.

The adapter bridges **CLIPBOARD and PRIMARY** through the same Wayland seat selections and keeper;
it does not implement drag and drop. Selection ownership is watched through XFixes notifications,
not polled. The bridge's hidden X11 owner window, transfer properties, generations and pending
requests all live inside the adapter. A Wayland offer originating from X11 is tagged so it cannot
become a new X11 owner and echo back. Changing owner, destroying either endpoint or restarting
Xwayland cancels that generation's transfers without clearing a newer selection.

- **X11 → Wayland:** asynchronously request TARGETS, then publish the supported MIME offer. On a
  Wayland reader's request, send ConvertSelection with a transfer-specific property. SelectionNotify
  identifies success or refusal; GetProperty replies are polled through the existing request queue.
  A normal reply is written to the reader's non-blocking fd. For **INCR**, acknowledge the initial
  marker by deleting its property, then read each PropertyNotify/NewValue chunk and delete it only
  after the bounded chunk has been accepted by the Wayland pipe. A zero-length chunk completes it.
- **Wayland → X11:** own the corresponding X11 selection while its Wayland source exists. Answer
  TARGETS and TIMESTAMP from cached metadata. For SelectionRequest, ask the Wayland source to send to
  a pipe and read it asynchronously. Small content uses ChangeProperty and SelectionNotify. Large
  content starts INCR and sends SelectionNotify; each PropertyNotify/Delete permits one bounded
  chunk, with a final zero-length property and no wait for the client in the loop. Use the requested
  property, or the target when the request specifies None, and refuse unsupported targets explicitly.
- Translate UTF8_STRING, STRING and text/plain MIME types explicitly; preserve other advertised MIME
  names as atoms. Cap simultaneous transfers and per-transfer buffers; stream rather than collecting
  an unbounded selection. Request and transfer-progress deadlines exist only while work is pending.
  Refusal, invalid type/format, dead pipes, stalled INCR handshakes and disappearing owners fail only
  that transfer. Do not fake MULTIPLE support before its independent subrequests are implemented.
- Real-client cases must cover both directions for CLIPBOARD and PRIMARY, small and INCR-sized data,
  split chunks/backpressure, refusal, owner replacement, endpoint death, timeout, Xwayland restart,
  persistence through the existing keeper and continued isolation from the parent desktop clipboard.

### Keeping the app tail small

Apps will do odd things with their windows, Steam and Proton above all. The tail can't be avoided, but
each oddity should be harmless to the session, found by us before the user meets it, fixed in one place,
and fixed once. Seven rules, the first three binding on the code from the start:

1. **Enforce, never negotiate.** The compositor tells a window its size and doesn't wait for agreement:
   a window that ignores it is scaled, positions it asks for are ignored, there is no stacking to request,
   and nothing waits on a client's acknowledgement. A misbehaving client can only make its own window look
   wrong, never placement, the other screen or the session.
2. **A client's error stays the client's.** Anything a client sends that we can't handle ends that
   client's connection, logged, never a crash. Xwayland is a supervised child: if it dies it is restarted;
   the X11 apps go, the session and the Wayland games stay.
3. **X11's oddities in one place.** The X11 adapter is a translation from a window's X11 properties to the
   same window facts the Wayland side produces (role, parent, size limits, title, identity, shown or not);
   the state never sees X11. An app quirk is a fact derived wrongly, fixed in that translation, and tested
   with the recorded properties as input and the expected facts as output, without running the app.
4. **The user can always get out.** Home goes through our pad router before any client sees it: Home
   returns to the shelf, Home+B closes the window, held it ends the program's scope, whatever the window
   does. The worst case of an app bug is pressing Home.
5. **Bugs fixed once stay fixed.** A set of real clients in the headless harness (a Wayland app, an X11
   app, a fixed-size dialog, a window ignoring its size, one that hides and reappears, a menu), growing by
   a case with each app bug fixed. On the device, a fixed-size ring of the last few hundred window events,
   dumped by a control command, replays as a state test; the maintenance agent can attach a dump to a
   report.
6. **Defaults that avoid the hard paths:** games through Wine's Wayland driver, so most game traffic never
   reaches Xwayland, and Steam in Big Picture mode (`-gamepadui`): one fullscreen controller-driven window
   instead of the desktop client's many windows and menus.
7. **Later, once the harness exists:** a randomized client that maps, unmaps, resizes, asks for
   fullscreen and opens menus, with the session's invariants checked after every step, as the randomized
   state test does one layer up.

### Frame statistics and limiting

**Decided: under the compositor, frame statistics and the frame-rate cap come from the compositor, and
MangoHud isn't used.** Today ClamShell launches games with MangoHud as a layer inside them: its CSV log
feeds the FPS tile, its `fps_limit` is the per-game cap (`Setting::FpsCap`), its overlay is the HUD
setting, and Waydroid's apps, which MangoHud can't reach, get the same log written from their commits.
The compositor sees every frame of every client without injecting anything: native Wayland apps, X11
games through Xwayland, and Android apps (Waydroid's windows are Wayland surfaces).

- **Statistics, per window:** when each frame was committed (the app's frame rate), when it reached the
  screen (the page flip's timestamp from DRM, also under direct scanout), frames committed but replaced
  before being shown (rendering faster than the display), and frames shown late (stutter where the user
  sees it). Commits arriving late means the app is slow; commits on time but shown late means it is ours.
  GPU and CPU load stay where they are (the kernel's per-client DRM statistics, `/proc`). Kept in a
  fixed-size ring per window, read by the FPS tile and the system info, no allocation per frame.
- **The cap:** the compositor paces a capped window's frame callbacks and presentation feedback to the
  cap, which FIFO clients follow (Mesa's Vulkan WSI, EGL with a swap interval, Xwayland's Present, and
  the fifo and commit-timing protocols). Clients that don't wait (Vulkan mailbox or immediate, EGL swap
  interval 0) can't be paced from outside, so a capped launch also forces FIFO through Mesa's settings
  (`MESA_VK_WSI_PRESENT_MODE=fifo`, `vblank_mode`). Together with the 120 Hz command-mode panel (section
  7), a capped game costs the panel link only the frames it sends.
- **The HUD:** the frame rate and frame time drawn by the compositor as an overlay element when the
  setting asks for it; the full MangoHud overlay goes, the details being on the second screen's system
  info, which can show them live beside the game.
- **Under sway,** for as long as it is kept, MangoHud stays as it is: sway gives nothing comparable. It
  goes with sway.

### HDR: later, kept possible

HDR is wanted, as the experiment in `docs/thor-gameplan.md` describes (the panels do no tone mapping of their
own: unlike an HDR monitor they take no PQ signal or metadata, and show the values sent at the brightness
set, so HDR is done as on phones: the panel driven brighter, ordinary content drawn dimmer within it, highlights in the headroom). It is not in
the MVP, but three choices keep it possible without rebuilding:

1. **No 8-bit assumption in composition.** Output buffers, the formats our elements produce and the
   compositing shader take their precision from configuration, ideally compositing in linear light, so a
   10-bit (2101010, which the display planes take, rotated too) or half-float path can be switched on.
   ClamShell's CPU-drawn UI stays 8-bit: SDR content placed within the range.
2. **A surface's colour description has a place** in our window data, though ignored for now: games
   declare HDR content through the Wayland colour-management protocol (Proton and DXVK through Wine's
   Wayland driver). Whether Smithay implements it is checked when its revision is pinned; if not, it is
   ours to add later.
3. **Plane assignment leaves room for a colour transform:** scanning out an HDR game means the display
   hardware's colour pipeline (Armada's colour-pipeline patches, phase 4's "Direct scanout") does the
   brightness and tone mapping, not our shader.

The pinned Smithay revision (24647fc) has no colour-management protocol implementation. Window facts
now reserve a colour description; the nested host does not advertise HDR. Client textures retain their
imported format. Its EGL backend negotiates 10-bit output with an 8-bit fallback and logs the result;
explicit precision configuration and linear-light composition remain work before an HDR path is enabled.
There is no plane assignment yet.

The work is split in two phases (section 6): 2h builds what is the same everywhere (the colour-management
protocol, composition in a common colour space, colour-aware scanout) on the desktop, nested on an HDR
monitor; 4h does what only the device can (the panels' brightness and headroom, tone mapping to their
measured peak, matching the two panels' white points for dual-screen emulators, an HDR setting per game).

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

## 6. Phases (as much as possible without the device; sway kept working throughout)

The state's window model changes first, under sway: the state speaks one model, and gets two executors,
the compositor (placing synchronously) and sway (placing a moment later with its rules, as today). The
sway-specific mechanisms (the landing workspace, `for_window` rules and their switch variables, marks,
stray-window matching) move out of the state into `crates/system/src/sway`, which approximates the model.
Steps 0-3 need no Thor; step 4's first checks can run whenever it arrives, alongside them.

0. **Prepare under sway, no Smithay.** Behaviour unchanged; worth doing whatever is decided about the rest.
   - [x] `ui::Core`: the state, the event queue, the input machines, timers as deadlines (not
         `LoopHandle<App>` timers), each screen recorded and drawn into a pixmap; `ui::platform` reduced to
         the Wayland host around it.
   - [x] Input routed by `Screen`, not by `WlSurface`.
   - [x] `Effect::Outputs { top, bottom }` and `Effect::Refresh(hz)` computed by sync, replacing
         `Effect::Screens`, `BottomPower` and the refresh read out of `Apply`/`Launch` in `app.rs`.

   Done when the pixel and focus references are unchanged, the trace differs only by those effects, the
   no-alloc tests pass, and the nested-sway test and the VM suite pass.
1. **The state's new window model, under sway. Done, 2026-10-23.** Window events (`Opened`, `Closed`,
   `Title`, `Activate`)
   and each window's place in the state; `crates/system/src/sway` translates sway's snapshots into those
   events and the places into sway commands, holding the rules, marks and landing workspace. Checked by the
   state's tests, the random test, nested-sway and the VM suite.
   - Implemented: lifecycle events and atomic batches, synchronous placement, retained home groups below,
     explicit window selection, independent keyboard focus, and parent/identity facts. Sway converts
     snapshots into sparse changes and executes placement with retry/reconnect reconciliation.
   - Foreground launch intent and short replacement history remain as backend-independent behavior.
     Placement effects remain for sway; the compositor reads the same committed state directly.
   - Validated: `tools/check.sh` (including 342 state tests, randomized sequences, allocation checks,
     snapshots and rustdoc), nested sway (0 failed), and the VM suite (119 passed, 0 failed).
     Pixel references are unchanged. Focus references now keep the pad on shelf when a scope has no
     window; trace changes record lifecycle inputs and selection at window arrival rather than scope
     arrival. Phase 2 can begin.
1b. **The two screens' window model, under sway. Implemented.** Section 3, "Windows on the two
    screens" and "The Home chords", supplies the placement read by phase 2's window presentation.
    Phase 1 was built against an earlier model with several windows below and tabs there.
    - [x] State: at most one window below; moving one down sends the one there back up into its program's
          windows on top; activation brings windows forward on top only; what each screen shows when a
          window closes; switching a program's windows on top; Home held known to the state.
    - [x] Pad router: Home + right stick flicks (down / up / left / right, once per flick past ~70%,
          re-armed below ~30%, a stick off-centre at Home's press must return first, the game sent a
          centred stick while Home is held), Home + right stick press for the second screen's view
          (replacing Home+Select), the screenshot staying on Home+R1; Home+Up/Down and Home+Select go.
    - [x] The sway adapter: keep its top tab bar instead of drawing arrows; no tabs on the second screen.
          Home-held edge arrows belong to phase 2's in-process compositor.
    - [x] docs/state-machines.md and tests for each rule; the random test's invariants (at most one window
          below, and the rest); the focus and trace references where they change, reviewed.

    Validated with unchanged pixel, focus and trace references, nested sway (0 failures), and the VM
    suite. The full VM run passed the new pad/window scenarios; two outdated test expectations were
    corrected and the affected scenarios rerun (36 passed, 0 failed).
2. **Done (2026-10-24), with three items deferred (below).** **The compositor, nested and headless,** implementing the same model synchronously, with the
   presentation of windows that aren't fullscreen (section 3): xdg-shell, Xwayland
   with the X11 adapter, input routing, the on-screen keyboard through text-input, the clipboard,
   screenshots, idle-inhibit. Developed with `clamshell --nested` on the desktop (real Steam and Proton on
   x86), tested headless under `cargo test`. sway stays the session's compositor.
   In progress: the nested host renders real Wayland clients, reads committed placement directly,
   configures fixed-size clients, centres native-sized windows, fits games, and uses the same transform
   for rendering and pointer coordinates. Home-held arrows use the shared state selection rule.
   A socket-pair protocol test checks configure/lifecycle behavior; `tests/nested-compositor.py` checks
   real EGL rendering, down/up and Home under a headless sway host. Touch routing keeps each contact
   on its initial surface and inverse transform,
   including simultaneous contacts on different screens. The on-screen keyboard commits text through
   text-input-v3 and sends shortcuts and other keys through the seat, retaining physical modifiers.
   Visible client idle inhibitors feed the existing idle state machine. Clipboard and primary selection
   use Smithay's selection protocols; the existing keeper connects to the private nested display and
   preserves text separately from the parent. A 256-entry window-input ring exports through the control
   command and replays against state fixtures. Real Wayland clients cover fixed-size dialogs, ignored
   configure and unmap/remap; a malformed client is isolated from a healthy one. Real popup clients
   cover nested grabs, outside touch dismissal, Home and parent unmapping; menus stay local to their
   toplevel and do not enter state. Cursors are composed by the host from cached xcursor theme frames
   or client surfaces, with cursor-shape requests, hidden cursors and hotspots covered by real clients
   and pixel checks. Xdg activation delegates recent, single-use tokens through state selection;
   expired/background requests and Home revocation are covered by a real client. The shared relative
   pointer/constraint path is tested headless with injected relative input, including camera motion,
   confinement, unmap/remap and Home release. The absolute-only nested host omits those protocols;
   the device will receive relative input from libinput. Xwayland now has an asynchronous private
   transport and supervised child, started by the first connection to its reserved display socket.
   Socket readiness and pidfd drive it; there is no idle liveness probe or polling deadline. Setup,
   write and outstanding-reply timeouts remain. X11 toplevels are translated to the same window facts
   (class, title, XRes PID, parent, size limits and role), placed before mapping, and use shared drawing,
   input geometry and feedback. A real X11 fixture covers top/bottom moves, resize, close, unmap/remap
   and lazy restart after a crash, while Wayland survives. Property fixtures cover malformed metadata
   and ordinary/dialog/splash/utility facts. Override-redirect menus and tooltips now share their parent’s scaling, screen clip and input
   geometry, with real-client dismissal and submenu tests. The clipboard bridge now imports and exports both seat selections with INCR and bounded pipe
   backpressure, covered by native clients, source/reader failures and Xwayland restart. Real X11 focus fixtures exercise all four ICCCM input models, matching nonzero focus timestamps,
   actual key delivery, Home release and unmap/remap. Drag and drop is outside this adapter's scope.
   Viewporter cropping and destination sizing use the
   same logical geometry for presentation and input, covered by a socket-pair client. Screenshots read the presented nested scene and
   encode it on the system worker; SDR PNG export does not change composition precision. The nested
   host advertises renderer-native dmabuf formats, with render-device feedback when available, and
   validates imports before drawing. The smoke harness also passes with `--gpu`, requiring actual
   native dmabuf submissions and checking presentation through moves and close. Alongside the EGL
   fixture, Wayland `vkcube` checks Vulkan WSI animation through screen moves and resizes, and requires
   complete dmabuf feedback plus native buffer submissions. The current host puts the logical
   screens side by side for desktop testing. Presentation feedback follows successful submissions
   and renderer-visible surfaces, with software monotonic timestamps and no hardware/vsync claims;
   real-client tests cover committed, superseded, hidden and unchanged content.
   Rules 1-3 of section 3, "Keeping the app tail small", hold for all of it; rule 5's real-client set
   and window-event ring, and rule 6's defaults are part of this phase. Steam's packaged desktop entry
   and game-start command now request Big Picture.
   Deferred, none blocking: the per-frame allocation ceiling in the headless harness (Smithay allocates
   per frame, so a pinned count rather than zero, with per-frame CPU time); Wine's Wayland driver as the
   launch default, after trying a few Proton games under sway and nested; the real Steam and Proton
   check by hand (Big Picture, a Proton game under X11 and the Wayland driver, clipboard, menus; the
   desktop's own Steam quit first).
2h. **HDR, the shared half, nested on the desktop** (section 3, "HDR"; after phase 2, beside phase 3;
    the desktop has an HDR mini-LED monitor under KWin):
    - [x] The colour-management protocol (`wp_color_management_v1`, parametric descriptions; no ICC
          profiles) on the server side, ours: Smithay has only the bindings. Surfaces' descriptions in the
          window data, as constraint 2 left room for. Clients are told the output's real headroom (the
          host's peak and average anchored to our reference white), not the content's current volume.
    - [x] Composition in a common colour space: each surface converted to linear light in the shader,
          a 10-bit or half-float output, ClamShell's SDR UI and SDR clients placed at a reference white,
          and the output's primaries applied (on the Thor, sRGB mapped into the top panel's wider gamut;
          the panels' colour facts come from the device profile, `docs/device-profiles.md`; nested, from
          the host's preferred description). Done nested: a half-float linear BT.2020 scene at reference
          white, encoded once for the output. On the DRM backend each screen is encoded for its own panel
          and described to clients as it (each output its own description and identity; the preferred one,
          which surfaces are told, the top's), from the device profile: the panel's primaries, gamma 2.2,
          its peak, and where SDR white goes (`sdr_white`; at the peak, no headroom, until the panel is
          driven brighter in 4h). Above a panel's peak a pixel is brought down whole, keeping its hue. The
          VM's `vm-hdr` profile gives the virtual screens the Thor's panels with SDR white at 203 nits on
          the top, for the HDR paths on DRM.
    - [x] The curve on every path, the owner's decision (2026-10-25): SDR and HDR content, HDR and SDR
          screens alike, in two stages. Per window, content to scene, limited by its screen's headroom:
          SDR's signal through the curve with white at most the screen's peak and highlights expanding no
          further (`Curve::map_on`); HDR's light (PQ, scRGB) with the black point and contrast on each
          channel below white (gamma 2.2 standing in for a signal), white at the curve's, and its own
          highlights compressed from the knee (the curve's start, at most three quarters of the peak) into
          the screen's peak when its peak (MaxCLL, else the mastering or nominal maximum) is beyond
          (`curve::map_hdr`); without a curve, an HDR window only compressed. Per screen: the gamut,
          the hue-keeping limit, the encoding. On an SDR screen a curve leaves its black point, contrast
          and saturation; the shaders' arithmetic is tested against the state's on SDR, 650-nit and
          unlimited screens. The compositor tells the state each screen's description as clients get it
          (`Reading::Screens`; nested, the host's real peak), and the curve is limited by that, not by the
          signal sent. The edit screen plots what the game's screen shows: an SDR game's signal, or an HDR
          game's PQ signal up to the peak it declares, with lines at the screen's peak, paper white and
          black, labelled in nits. A game's declared peak (MaxCLL, else its mastering display's) is often
          missing or a fixed value; without one PQ's 10000 nits is assumed, which compresses more than
          needed but never clips. The program's setting "HDR content peak" replaces it (as declared by
          default; 400 to 4000 nits), whatever curve is chosen: it was first a curve parameter, which a
          shared curve carried to every game using it (2026-10-25); the edit screen's header says the
          content's signal and where its peak comes from.
          Measuring it from the frames is for later, with the black floor.
    - [ ] Scanout knowing about colour: a surface whose description matches the output passes through
          untouched (constraint 3). With the DRM backend (phase 3): the nested host has no scanout.
    - [x] SDR to HDR for games (inverse tone mapping, as Special K does rather than Windows' Auto HDR): a
          per-pixel curve on luminance, hue and saturation kept, no object classification. Parameters, per
          game: the black point (many games lift their blacks; mapping the game's floor to true black suits
          the OLED), shadow contrast, the SDR white the midtones stay anchored to, where highlights start
          expanding and the peak they expand to, saturation. Off by default. Optionally later: the black
          floor measured from the frames (the darkest values over a long window) to suggest the black
          point, and the curve adapting slowly to each frame's average and peak.
          Edited as keymaps are (the owner's decision, 2026-10-25): one tile in the game's settings, the
          HDR curve, opening a picker over the settings tab like the keymap picker: the game's own curve
          (or a row making one from the curve in force), Off, the named curves with how many games use
          each, New; menus with Edit, Rename, Duplicate, Delete, and for a game's own Load from and Save
          as. Presets to start from (Neutral, True black, Punchy). The edit screen has a slider per
          parameter, applied live while the game runs on top. The list of named and own profiles, their
          storage, the picker and the menus become one implementation shared by keymaps and curves, not a
          copy. Steps: (1) the profile list (named and own, ids, names, versions, own-from, load, save as)
          generic, keymaps on it, behaviour and trace unchanged; (2) storage the same way; (3) the picker
          and menus shared; (4) curves: the parameters, presets, the setting (a curve's id, 0 Off), the
          tile, the edit screen; (5) the compositor applying a window's curve to its SDR surfaces, read
          from the state; under sway the tile is dimmed (the compositor alone can do it). Done: the
          profile list, storage and picker shared with keymaps (`profiles`), `curve` with its three to
          start from, the tile (Upscale's left out until upscaling works, to keep four rows), the edit
          screen with the curve plotted on PQ's scale, and the compositor's SDR input shader applying the
          window's curve from uniforms (its arithmetic tested against `Curve::map`). Left: trying it on
          real games, the black floor measured from frames, adapting to each frame.
    - [x] The nested output: the host window's own colour description given to KWin on its raw Wayland
          surface (winit has no colour management), so KWin tone-maps our output to the monitor.
    - [ ] Tests: headless, a PQ client's pixels converted as expected, SDR content unchanged, the UI at
          reference white (done: unit tests and PQ, linear and sRGB EGL fixtures in the nested test);
          nested, mpv (`gpu-next`, `--target-colorspace-hint=yes`) playing a PQ clip, compared by eye with
          the same clip in mpv directly on KWin (done 2026-10-25: they look the same) (a synthetic clip: a 0-1000 nit ramp, patches from 1 to
          1000 nits, a moving 1000-nit highlight); a Proton game with HDR through Wine's Wayland driver.
3. **The generic DRM backend, on the VM:** udev, DRM, GBM, GLES, libseat, libinput, two outputs, power,
   refresh. The VM image boots either compositor; the VM suite checks both. (dmabuf feedback with scanout
   tranches moved to phase 4, with direct scanout.)
   - [x] `clamshell --drm`: libseat, DRM and GBM with GLES, libinput, both screens from the device profile
         (the kernel's mode kept, turned as mounted), each shown its part of the one composed scene and
         drawn only when that part changed; flips give presentation feedback and frame callbacks; VT
         switching. Tried in the bring-up image (llvmpipe): both screens right, about 1 % CPU idle, clicks,
         touch, keys, a Wayland client (foot) shown and moved to the second screen.
   - [x] The session switch: `/etc/thor/session` saying `clamshell` runs it on tty1 instead of sway.
   - [x] Output power and refresh as the state asks (`Effect::Outputs`, `Effect::Refresh`); touch on
         panels mounted turned (the Thor's), rotated with the output; ClamShell's package built with the
         compositor (its libraries in the aarch64 sysroot). The render device's dmabuf feedback is sent;
         the scanout tranche waits for direct scanout (phase 4).
   - [x] The session's services under the compositor (udiskie; notifications are phase 6), and the
         watchdog and restart that clamshell.service gives under sway. Done: thor-session starts
         `clamshell --drm` through uwsm, as sway (`uwsm start`); ClamShell runs `uwsm finalize` once it
         listens, which gives the user manager WAYLAND_DISPLAY and DISPLAY and brings up
         graphical-session.target, so udiskie starts; clamshell.service and thor-mako.service start only in
         sway's session (`ConditionEnvironment=XDG_SESSION_DESKTOP=sway`). No watchdog or systemd restart:
         a compositor restarting takes every window with it, so thor-session's count (3 ends within 2
         minutes) stays the guard. Tried in the VM, both sessions.
   - [x] Frame statistics and the cap ("Frame statistics and limiting", section 3) on the DRM backend's
         page-flip timestamps: the per-window ring, the FPS tile and system info reading it, the cap
         pacing frame callbacks with FIFO forced at launch, the HUD as an overlay element; MangoHud not
         launched under the compositor. Tested headless with injected flips and in the VM with `--gl`
         (`vkcube` and a capped client); exact pacing measured on the device (phase 4). Done: a frame is
         a new buffer on the window's surface, counted when a flip (nested: a submission) shows it; the
         top screen's window gives `Reading::Frames` every half second while it draws. vkcube in the VM
         reads 60 fps uncapped, 30 and 40 capped (40 on 60 Hz: two refreshes, then one), the cap
         following the tile live. "HUD FPS" shows the frame rate, "Full HUD" adds the frame time.
         Not done: frames replaced before being shown and frames shown late (no view reads them yet);
         Waydroid's Wayland filter still writes its frame log, which nothing reads under the compositor.
   - [x] `tools/vm/vm.py --gl`: the x86_64 VM with `virtio-gpu-gl-pci` (`venus=on,blob=on,hostmem=...`)
         and `-display sdl,gl=on` (`egl-headless` when headless), so the guest has OpenGL through virgl
         and Vulkan through Venus (the guest's `vulkan-virtio`), both on the host's GPU. The compositor's
         VM checks run with it: GLES and GBM on real hardware, dmabufs from GPU clients, and a Vulkan
         client (`vkcube`) through the WSI path. Without `--gl` the VM stays as it is (pixman, no host
         GPU), and the existing suite keeps running that way. Confirmed with QEMU 11.1 and virglrenderer
         1.3 on a Radeon RX 9070 XT: ClamShell's GLES renderer is virgl on it, `vulkaninfo` shows Venus
         (Vulkan 1.4), and `vkcube --wsi wayland` animates under the DRM backend. QEMU's `screendump`
         has no surface for a GPU scanout, so with `--gl` the screens are seen through ClamShell's own
         screenshot (`clamshell ctl screenshot`, both screens side by side).
4. **On the device:** the checks of section 5 (niri as a stand-in, possible as soon as the Thor arrives),
   then the Snapdragon specifics: panel orientation, rotated plane scanout, command-mode pacing, the
   refresh policy, the brightness and power guards; measurements (frame statistics against MangoHud's
   under sway for the same game, a capped game's pacing and power); daily use with sway still the default.
   - [ ] Direct scanout and the scanout tranche. The DRM backend now always composes (the colour
         pipeline: curves, the panel's colour), so no client buffer reaches a plane, and a scanout tranche
         would only steer allocations for nothing. Direct scanout needs the backend on Smithay's
         `DrmCompositor` (it picks planes, test-commits, gives per-surface scanout feedback), and is
         allowed only when composition would change nothing the display engine can't do itself: the
         window fullscreen on its screen, no overlay over it, no FSR, SMAA or Integer pass.
         - The colour work can move to the display engine. Linux-thor carries Armada's patches for it
           (series at d2bcf6f; the SM8550 device tree included): 0068 loads the DSPP's inverse gamma
           (`DEGAMMA_LUT`) and 17^3 gamut LUT (CRTC property `DPU_3D_LUT`), after blending, through the
           LUTDMA engine; 0077 gives each VIG pipe a DRM plane colour pipeline, a 1D LUT then a 3D LUT,
           before blending. The panel's correction and gamut mapping become the 3D LUT, the curve the 1D
           LUT (or baked into the 3D one), HDR tone mapping a PQ 1D LUT then the 3D LUT, as gamescope
           does (Armada saw gamescope's shaper and 3D LUT scan out on the Odin 2 Portal, the same SoC).
         - Limits: a plane with a colour pipeline needs a VIG pipe and no multirect (the second rect has
           no LUT blocks); a rotated top-screen plane already takes two of the four VIG pipes. Smithay
           doesn't know the plane colour pipeline API: its properties set by us and included in the
           test commits (in our fork, or around `DrmCompositor`).
         - First read on the device: `drm_info` (the planes' colour pipelines and formats, the CRTC's
           `DPU_3D_LUT`), whether a rotated VIG plane with a colour pipeline at 120 Hz passes a test
           commit, the top panel's gamut against sRGB, and the power composition costs against scanout;
           then decide. With it, the per-surface feedback's scanout tranche (UBWC for Turnip and
           freedreno, section 4).
4h. **HDR, the device half** (after phase 4's measurements, as the gameplan's HDR experiment describes;
    the panels do no tone mapping of their own, so ours does it all):
    - [ ] Bit depth. The display controller takes 10-bit buffers (2101010), but the link to both panels is
          8-bit as the drivers set it today (`MIPI_DSI_FMT_RGB888`, `bpc = 8`; the top's DSC at 8 bits per
          component). Find out whether the controller dithers 10-bit content down to 8 bits for them, and
          whether the panels' controllers (ICNA3520, CH13726A) accept 10 bits (DSI RGB101010, DSC at 10
          bits per component) with a driver change; AYN's Android may show which it uses.
    - [ ] Start from what gamescope already does on the top panel (Armada PR #261, merged 2026-08-20,
          tested on a Thor): its display profile marks the ICNA3520 HDR-capable with a gamma 2.2 EOTF, so
          gamescope tone-maps itself and sends ordinary pixel values; primaries R 0.680/0.320, G
          0.265/0.690, B 0.150/0.060, white D65; 650 nits peak and frame average, 0.002 black. The same
          primaries mean SDR content needs mapping from sRGB to the panel's wider gamut to look right,
          which 2h's composition can do for SDR alone. The bottom panel (CH13726A) has no profile there.
    - [ ] The panels measured: real peak brightness, banding, heat at high
          brightness; whether HDR is worth having on them at all. If not, 4h stops here and 2h's work
          still serves SDR colour correctness.
    - [ ] The output: the panel driven brighter for HDR content, SDR content dimmed to keep its look,
          highlights in the headroom; our own tone mapping into the measured peak; limits on highlights
          against heat and burn-in.
    - [ ] The two panels' white points matched (dual-screen emulators); an HDR setting per game.
    - [ ] HDR games through the same curve, not a second setting: a curve is how a game should look on
          the screen, applied to whatever it sends. For SDR the highlights expand into the headroom (now);
          for an HDR game (its own HDR mode through Proton, Wine's Wayland driver and colour management)
          they are compressed into the panel's peak from the same knee, with the black point, contrast,
          paper white (SDR white) and saturation meaning the same. Off for an HDR game is plain tone
          mapping into the panel. The edit screen plots whichever applies to the game's output. Tried
          first on the desktop with a game that has an HDR mode (2h's Proton test). The compositor's half
          is done (2h, "The curve on every path"); left: trying it with real HDR games, and on the panels.
    - [ ] SDR to HDR on the device: 2h's curve into the panel's headroom; in the display hardware's
          per-plane lookup tables if they can hold it, so a game keeps direct scanout, else in the shader
          (every frame composited: measure the cost); highlight limits against heat and burn-in, static HUD
          elements in particular.
4p. **Picture** (`docs/picture.md`): the curve as the colour profile, and Resolution, Scaling (Bilinear,
    Nearest, FSR 1), Anti-aliasing (SMAA) and Sharpen as setting tiles, applied per window in the compositor; and
    OptiScaler for Proton games' own upscalers and frame generation, once a test on the device shows it
    works on Turnip. Its first phase is on the desktop, its measurements on the device.
5. **The default switches** to ClamShell's compositor; sway stays installed and selectable as the fallback
   for as long as the owner wants it. Dropping it (the sway adapter, its tests, the sway packages) is a
   later decision, taken once the built-in compositor has proven itself in daily use.
6. **Absorb, later:** the clipboard and the Waydroid proxy in-process, libinput replacing the evdev watch
   thread, notifications, uwsm, refresh calibration from vblank timestamps, direct injection of the pad
   mouse and keymap keys.

## 7. Decisions still open

| Decision | Recommendation |
|---|---|
| Xwayland: satellite or built-in X11 WM | decided: in-process async X11 adapter using x11rb; the pinned Smithay XWM blocks on replies (section 3) |
| Notifications without mako (no layer-shell): ClamShell's own low-battery warning reaches nothing otherwise | ClamShell draws its own notices on the top screen, and serves the notification D-Bus API itself later; minimal layer-shell (about 250 lines) only if third-party overlays turn out to matter |
| The top screen's tabs: sway shows a tab strip when a group has several windows | none on the top screen; switch a group's windows from the second screen; dialogs drawn over their parent |
| The top panel's refresh: a change blanks it for 0.4 s | stay at 120 Hz and limit a game's frame rate: in command mode the host sends only the frames committed, so a 60 fps game at the 120 Hz mode costs the host what it would at 60 Hz; what remains is the panel scanning its own memory at 120 Hz. Measure the battery at both modes with the same 60 fps game and a static screen; if the difference is small, drop the 60 Hz mode; if it matters, look for a seamless switch of the panel's internal rate (a vendor command, a kernel driver change) rather than a blanking modeset |
| Frame statistics and the frame-rate cap | decided: from the compositor for every client, Android included; MangoHud only under sway, and gone with it (section 3) |
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
- The real-client set and the replay of recorded window events (section 3, "Keeping the app tail
  small"); each app bug fixed adds a case. Later, a randomized client with the session's invariants
  checked after every step.
- A control socket with replies (`ctl windows`, `ctl screenshot FILE`) for the VM suite and any remaining
  shell test.
- The VM suite keeps DRM on virtio-gpu, libinput, logind and the session's files; the compositor's checks
  run with `--gl` (GLES and Vulkan on the host's GPU through virgl and Venus), the rest without. virtio-gpu
  has no planes and mostly linear modifiers, so the device covers rotation, both panels, refresh, power,
  scanout, modifiers and frame pacing.

## 9. Size

Revised 2026-10-24 after implementing the asynchronous XWM and clipboard bridge. The range includes
socket activation and both clipboard directions, excludes drag and drop, and keeps tests and fixtures
additional to production code. The compositor crate currently has about 5,500 production lines,
excluding inline tests; about 2,700 belong to the X11 adapter. Rust tests and client fixtures add about
4,500 lines, the real-client Python harness about 550, and property fixture data about 350. This is a
planning range for the complete MVP, not a claim that all remaining work is implemented.

| Part | Lines | Effort |
|---|---|---|
| UI core extraction (phase 0) | ~600 changed | low |
| Event loop, Smithay state and protocol handlers | ~1,200 | medium |
| UI texture element with damage | ~250 | low |
| Input routing, touch mapping, focus, cursor | ~750 | medium |
| DRM backend: two rotated outputs, power, mode change, vblank pacing, scanout | ~1,200 | high (the device risk) |
| Nested and headless backends | ~550 | low-medium |
| Xwayland (async X11 adapter, activation, supervision and clipboard bridge; no drag and drop) | ~2,500-3,500 | high (async transfers and app quirks) |
| Placement, configure sizes, dialogs, close | ~500 | medium |
| Text input and key injection | ~250 | medium |
| Screenshot, idle-inhibit, clipboard, session | ~400 | low |
| The state's window model | ~1,000 code, ~2,000 test lines changed | high (the logic risk) |
| Packages, calibration, self-test, VM suite | ~300 | low |
| **New production code** | **~9,000-11,000** | |
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
