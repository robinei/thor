# Device profiles: the steps (history)

The steps that moved the device's facts into `crates/device` (`docs/device-profiles.md`), done by
2026-10-26. Kept for the record; references are as they were.

1. **Implemented.** `crates/device` with `Profile`, the Thor's and the VM's constructors and identification; `Init`
   carries the profile; `CLAMSHELL_DEVICE` replaces the VM's environment overrides.
2. **Implemented.** `clamshell device outputs`, used by `thor-outputs` (then the VM branch there goes) and
   `thor-panel-calibration`. (2026-10-26: `clamshell device outputs` and `thor-outputs` went with sway;
   the compositor takes the screens from the profile, and the calibration uses `clamshell device panel`.)
3. **Implemented.** Each board fact moved, one commit each, its readers taking the fact. Detectable display modes were removed from the inventory of fixed facts.
   Moved: screen connectors, transforms, touchscreen mapping, backlight role hints, bottom-screen touchpad density, top-panel refresh calibration, panel colour descriptions, auxiliary/Back control mapping, driver button conventions, sensor-hub service/timing/axes, userspace fan policy, charging quirks/OEM switch names, and the keymap control diagram/printed legends, and requested preview/fixture sizes. Preview sizes are explicit drawing policy, never detected panel geometry. Refresh choices are the rates the DRM compositor reports at start, with no nominal rates when none are reported. Frame-cap values remain generic UI convenience policy (they do not claim output modes); inactive upscale choices use relative sizes rather than assuming a 1080-line panel. Standard battery controls remain detected on every application; only the known broken threshold overrides their use.
4. A rule in `AGENTS.md` (added with this plan): device facts live in the profile, code never checks the
   device.
