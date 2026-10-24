# Device profiles

How ClamShell keeps what is specific to one device in one place, so that another dual-screen clamshell
can be added without conditionals spread through the code. In progress after the compositor's phase 2,
in small steps. ClamShell keeps assuming two screens, a top and a bottom one.

## Rules

1. **Detect first.** What the hardware announces stays detected at runtime: the outputs' modes, refresh and
   physical dimensions, backlights, the gamepad by its capabilities and its stick ranges, force feedback, the battery,
   cpufreq clusters, the GPU's devfreq, thermal zones, hwmon, the power key and lid. A profile holds only
   what can't be detected. Reported or measured facts take precedence over profile hints; unknown
   values stay unknown rather than receiving nominal desktop or VM defaults.
2. **One place maps the device to its facts, in code.** A `Profile` is a struct of typed facts, built by
   one constructor per device (`Profile::thor()`, `Profile::vm()`, `Profile::generic()`). Facts can point
   at tables in code: the keymap screen's control layout, the fan curves, the sensor hub's axis matrix.
3. **The device is identified once.** At start, from the device tree's `compatible` (DMI on x86, or
   `CLAMSHELL_DEVICE` to force one), mapped to a constructor by one match. That is the only place in the
   codebase that checks which device it runs on.
4. **Everything else reads facts.** Each part receives the facts it uses: the pad thread its controls and
   gyro, the state its layout, choice lists and capabilities (through `Init`), the fan daemon its curves.
   Code asks "is there a fan curve?", never "is this a Thor?". A missing capability dims its UI, as
   elsewhere.
5. **Package scripts ask the binary** (`clamshell device outputs`, `clamshell device panel`) instead of
   repeating connector names. Later the compositor reads the profile directly.
6. **No file format yet.** A profile becomes data in a file only if outside profiles are ever wanted; the
   same struct could then be deserialised, without changing its readers.
7. **Two screens.** A profile always names a top and a bottom screen. A single-screen device would later
   make the bottom optional; nothing is built for it now.

## Where it lives

`crates/device`, a leaf crate with no dependencies and no I/O beyond what identification needs (reading
`compatible` stays in the callers: the system crate and the fan daemon). The state crate depends on it,
and through it the UI and compositor; the system crate and the fan daemon depend on it directly. It is
its own crate because the fan daemon, a separate service, needs its curves from it, and the state crate
already depends on the fan crate (its mode protocol).

## What moves into it

From the inventory of 2026-10-24 (paths under `clamshell/` unless noted):

| Fact | Today |
|---|---|
| The screens' connectors: top DSI-2, bottom DSI-1 (VM: Virtual-1, Virtual-2), both turned 90° | `src/config.rs:47` default `DSI-1`; `packages/thor-session/.../thor-outputs`; `packages/thor-system/.../thor-panel-calibration` |
| The touchscreen of each screen (`top_touchscreen`, `bottom_touchscreen`) | `thor-outputs` |
| Backlight per screen (`ae96000.dsi.0` top, `ae94000.dsi.0` bottom) | `crates/state/src/levels/mod.rs:91-97` |
| The bottom screen's density hint (16.5 px/mm), used only without reported physical geometry | `crates/system/src/wiring/mod.rs:52` |
| Top panel refresh calibration (ICNA3520 frame lengths) and its Device view row | `crates/system/src/display/refresh.rs`, `thor-panel-calibration` |
| Each panel's colour: primaries and white point, transfer function (gamma 2.2), peak, frame-average and black luminance, the link's bit depth (8 today: dither when compositing in 10), HDR capable or not; for 4h the brightness register's range and the level that gives the peak. The top panel's from gamescope's profile (R 0.680/0.320, G 0.265/0.690, B 0.150/0.060, D65; 650/650/0.002 nits); a panel without one is sRGB at gamma 2.2, SDR. Nested on a desktop the host's description is used instead | `docs/compositor-mvp.md`, phases 2h and 4h |
| The AYN key (`gpio-keys-ayn`, `KEY_F24`), Back (`BTN_BACK`) | `crates/system/src/input/pad/device.rs:36`, `remap/mod.rs:203-209` |
| The X/Y swap by driver (`rsinput`, `xpad`) | `crates/system/src/input/pad/device.rs:145-166` |
| The gyro: Qualcomm sensor hub, QMI service 400, 200 Hz, axis matrix | `crates/system/src/input/pad/sensor_hub/mod.rs` |
| The keymap screen's control layout and labels | `crates/ui/src/views/keymaps/pad.rs`, `words.rs:76-91` |
| The fan: hwmon `pwmfan`, its curves | `crates/fan/src/main.rs:26`, `curve.rs` |
| The charge switches (`retroid_*`) and the default hold method | `crates/system/src/power/tuning.rs` |
| The refresh, frame cap and upscale choices (1080-line panel, 60/120 Hz) | `crates/state/src/settings/mod.rs:313-318` |
| The nested window's and snapshots' screen sizes (the Thor's, halved for nested) | `src/session.rs:75-77`, `src/dev/snapshot.rs:47-53`, `crates/compositor/src/server.rs` |

What stays in per-device packages, as now: the kernel and its patches, the device tree, firmware, the
sound profiles (UCM), the motion sensors' services, udev rules, sleep settings, the boot layout and
command line, the self-test.

## Steps

1. **Implemented.** `crates/device` with `Profile`, the Thor's and the VM's constructors and identification; `Init`
   carries the profile; `CLAMSHELL_DEVICE` replaces the VM's environment overrides.
2. **Implemented.** `clamshell device outputs`, used by `thor-outputs` (then the VM branch there goes) and
   `thor-panel-calibration`.
3. Each hard-coded fact above moved, one commit each, its readers taking the fact.
   Moved: screen connectors, transforms, touchscreen mapping, backlight role hints, bottom-screen touchpad density, top-panel refresh calibration, panel colour descriptions, auxiliary/Back control mapping, driver button conventions, sensor-hub service/timing/axes, userspace fan policy, charging quirks/OEM switch names, and the keymap control diagram/printed legends. Standard battery controls remain detected on every application; only the known broken threshold overrides their use.
4. A rule in `AGENTS.md` (added with this plan): device facts live in the profile, code never checks the
   device.
