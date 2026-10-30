# Steam, PC gaming and Android: plan (2026-10-30)

This plan covers the three gaps the 2026-10-30 survey found: Steam integration, per-game PC-gaming tuning, and Android. Each area can be done on its own. For each area the plan gives the options, a recommendation, the flows with mockups, where the code goes, what changes in the settings model, the packaging, what only the device can answer, and the phases. The other systems were read in `/home/robin/.cache/thor-survey/`. Where this plan relies on how they work, it names the file.

## Summary

| Area | Recommendation | Rejected |
|---|---|---|
| Steam | Steam stays a program (Big Picture, `-gamepadui`). The first boot installs it once the device is online, and the Device view shows that and updates it, without a terminal. A launch hook hands every Steam game ClamShell's per-game settings. The settings tab on the second screen is our Quick Access menu. Later, games installing in Steam show in the Library. | Handing the screens to gamescope's Game Mode; Game Mode nested in a window; ClamShell speaking gamescope's Steam protocol; Decky; Steam Input as ClamShell's own feature |
| PC tuning | Three new settings, with the same All / runner / game levels as every other setting: x86 emulation (FEX presets), Vulkan driver (system Turnip or our newer build), Frame generation (2x, a Vulkan layer). All three go to the game through the environment, which reaches Steam games through the hook. Packages: `turnip-devel`, a frame generation layer, Proton-CachyOS ARM64. | A Proton tile in ClamShell (Steam's own Compatibility page does that job); individual FEX options; a game database; Auto TDP |
| Android | Stay with Waydroid. It is set up from the Device view in one of two ways: plain Android with F-Droid included, or Android with Google Play from Waydroid's own GAPPS image, which the device downloads, plus Google's registration step. The players' pads go into the container. | Lepton (Valve's Android layer): needs Steam, podman and an Android build of Mesa, has no Google apps, and runs Android 11. Patching Google's apps into the image or faking a certified device identity: not shippable. |

---

## 0. What the others do (read in their repos)

- **Kettle** (Valve's SteamOS "deckard" userspace plus Decky plugins).
  - Game Settings is a Decky plugin. It rewrites each game's Launch Options through Steam's JavaScript (`SteamClient`, `packages/kettle-decky-plugins/shared/launchOptions.ts`), putting `VAR=value %command%` in front of the game's command.
  - FEX options are `FEX_*` variables (`shared/game-options.json`: `FEX_TSOENABLED`, `FEX_VECTORTSOENABLED`, `FEX_SMCCHECKS`, ...).
  - Turnip from Mesa main is a second driver kept out of the Vulkan loader's search path (`/usr/lib/kettle-turnip`). A game picks it with `VK_DRIVER_FILES` (`packages/kettle-turnip/PKGBUILD`).
  - Frame generation is its own implicit Vulkan layer, `kettle-framegen` (BSD-3). It loads with `KETTLE_FG=1`, always 2x, and the game runs at half the refresh rate held by FIFO pacing (`game-settings/py_modules/framegen.py`).
  - OptiScaler is built for ARM64EC (`packages/optiscaler-arm64ec`).
  - Lepton: Valve's launcher (MIT) built from source, running on the Android image Steam downloads (app 3029110), in rootless podman, with Valve's Android build of Mesa (`packages/kettle-lepton/PKGBUILD`). Google Play is only APK downloads through apkeep; there are no Google services.
- **Armada** (Fedora bootc, gamescope-session-steam, Plasma).
  - Armada Control prefixes every game's launch options with `armada-game-launch %command%` through `SteamClient` (`decky/armada-control/src/lib/steamCompat.ts`).
  - That wrapper (`system_files/usr/libexec/armada/armada-game-launch`) reads per-game settings and applies them before the game starts:
    - FEX presets Default / Fast / Compatible (`usr/share/armada/fex-profiles.json`), through a `FEX_APP_CONFIG` file. Its comment: "the on-disk FEX config files are seen only by the system FEX, not arm64ec/wow64".
    - Turnip by `VK_DRIVER_FILES`: the Stable, Devel and WinNative builds (`packages/mesa/variants`) plus user-supplied ones. A vkd3d shader cache per driver.
    - Core affinity, the Wine CPU topology, and environment variables.
  - Proton-CachyOS ARM64 goes in `/usr/share/steam/compatibilitytools.d` under a fixed tool name (`packages/proton-cachyos`).
  - It has Lepton and Aurora (anonymous Google Play downloads), and Waydroid too, with an Android Mesa overlay only for SM8750 (`usr/libexec/armada/waydroid-mesa-overlay`).
- **PB-OS / SteamOS-ARM-Port** (Valve's SteamOS for the Steam Frame).
  - Their Quick Access tab is injected into Steam's UI over the CEF debugging port. It has to be re-checked after every Steam update (`external-and-mods/pbos-qam/STEAM-INTERNALS.md`).
  - Frame generation is lsfg-vk, which reads `Lossless.dll` from the user's purchased Lossless Scaling.
  - Google Play in Lepton is built on the device: MindTheGapps files fetched there, `services.jar` patched with smali, and the device presented as a certified Pixel 5 (`external-and-mods/pbos-android/README.md`).
- **Pocknix** runs gamescope on DRM with `steam -gamepadui -steamos3 -steampal -steamdeck`. It notes that "gamescope's modeset disables every other CRTC", so the second screen goes dark. It also warns that a global `CompatToolMapping "0"` makes Steam refuse to install Valve's tools (`packages/shared/pocknix-steam/pocknix-steam`).
- **Thorch** can run Steam in nested gamescope (`THORCH_STEAM_USE_GAMESCOPE`, `thorch-start-steam-arm64`).

---

## 1. Steam integration

### 1.1 Options

**A. Steam's gamescope Game Mode as a mode ClamShell hands the screens to.** ClamShell stops, or gives up DRM master, and gamescope-session-steam runs. "Back to ClamShell" reverses it.
- Gains: the real Quick Access menu over games, Steam's Performance panel, Decky, Steam Input as Valve ships it. What buyers who want SteamOS want.
- Costs:
  - gamescope drives one output: its modeset turns off the other CRTCs, so the bottom screen goes dark (Pocknix). Keeping it lit needs a second compositor on the bottom (Armada's bottom-gamescope, Kettle's Plasma Mobile).
  - Gamescope needs Adreno, rotation and touch patches. Every one of them carries its own.
  - The Performance panel needs Valve's steamos-manager, built for ARM and given device configs.
  - The "..." button needs a Steam Deck controller (InputPlumber, or Pocknix's uhid Deck).
  - SteamOS stubs: steamos-update, steamos-select-branch, jupiter-biosupdate, polkit helpers.
  - Switching takes a Steam restart, 20–60 s, with every game closed.
- Loses for as long as the mode is on: everything of ClamShell's. That is the Library (emulators, files, Android), the bar, keymaps with layers and gyro, the picture settings, the stick lights, the fan modes and the notifications.
- It is two sessions on one device, which the owner rules out.
- Known Adreno problem: the Quick Access menu freezes Big Picture (Steam #13544).
- Verdict: no.

**A'. Lease the top screen to gamescope and keep the bottom.** Smithay implements `wp_drm_lease_v1`, but gamescope can't run on a lease without patching. Its libinput would also fight ClamShell's for the touchscreens and keys. Verdict: no.

**B. Steam Big Picture as a program inside ClamShell's compositor (today: `steam -gamepadui -silent`).**
- Gains: Steam's library, store, downloads, friends and per-game properties (Proton choice, Steam Input) in one controller-driven fullscreen window. Games run as their own Library entries (the follower).
- Steam's Quick Access menu outside gamescope is thin: notifications, friends, music, help. Performance needs gamescope and steamos-manager.
- Over a running game it depends on Steam's in-game overlay being injected into the game. Whether that works for ARM64EC Proton games is unknown.
- Verdict: keep it as the base.

**C. Game Mode nested: `gamescope --backend wayland -e -- steam -gamepadui -steamos3` as one ClamShell window (Thorch's option).**
- Gains: Steam's overlay over games, since gamescope composites it. Steam's frame limiter. Decky is possible.
- Costs:
  - Every game and Steam's UI are one window. ClamShell's per-game picture settings, Size and Resolution land on gamescope's surface rather than the game's.
  - A second compositing pass and a frame of latency on the Adreno.
  - Two frame limiters, gamescope's and ClamShell's.
  - `-steamos3` makes Steam try to manage updates and Wi-Fi, which needs stubs.
  - The Performance panel is still missing without steamos-manager.
- Verdict: no. A user can still declare it as a runner in library.json; nothing of ours is needed for that, and it isn't supported.

**D. ClamShell speaks gamescope's Steam protocol.** ClamShell's Xwayland would expose gamescope's atoms (`STEAM_OVERLAY`, `STEAM_GAME`, `GAMESCOPE_FOCUSABLE_APPS`, ...) so that Steam in `-steamos3` mode puts its overlay over games inside ClamShell. The protocol is undocumented, changes with Steam and gamescope, and spans X11 atoms and Wayland protocols. ClamShell would turn into a gamescope imitation. Verdict: no.

**E. Decky without gamescope-session.**
- It is feasible in principle. Decky's loader injects through Steam's CEF debugging port (`.cef-enable-remote-debugging`) and needs no gamescope. Its tab appears in Big Picture's Quick Access menu.
- Its loader ships as an x86 binary. Kettle (`packages/decky-loader`, `packages/remora`) and PB-OS rebuilt it for ARM. Plugins with native x86 parts (PowerTools) break.
- Its UI is only inside Steam's window, never over a game in our session.
- Every Steam client update can break it (PB-OS's release check list).
- The debugging port lets any local process act inside the user's signed-in Steam (`docs/security.md` would have to accept that).
- What its popular plugins do, ClamShell already does (table below).
- Verdict: no.

**F. Steam Input through Steam, with ClamShell's pad passed through.**
- What happens if a user turns on Steam's Xbox configuration support:
  - Steam takes ClamShell's virtual Xbox 360 pad as a physical controller, applies the game's layout (community layouts for Xbox controllers included), and hides that pad from the game behind its own virtual pad.
  - With the game's Keymap at Default, ClamShell passes buttons and sticks through unchanged.
  - The AYN button is the game's Guide (`Button::Ayn`), so Steam's Guide chords work. Home stays ClamShell's.
- Loses:
  - Gyro: an Xbox 360 pad has none. Giving Steam one would mean ClamShell emulating a DualSense or a Steam Deck over uhid. That is a large job for something ClamShell's gyro keymaps already do.
  - Steam Deck community layouts, which are made for another controller type.
- Verdict: it works as Steam's own setting and needs no ClamShell UI. It is documented in `docs/runners.md` and checked on the device. The default stays as it is: Xbox support off, ClamShell's keymaps doing all the remapping.

**G. (Recommended) Steam is a program, and ClamShell is its Quick Access menu.** That is B, plus:
1. Steam set up and updated from ClamShell's Device view, with progress shown, in place of the foot terminal.
2. A launch hook so that every Steam game gets ClamShell's per-game settings, including games started from Steam's own UI.
3. Later, games installing in Steam shown in the Library.

**What Quick Access and Decky provide, and what ClamShell already covers**

| Quick Access / Decky | ClamShell |
|---|---|
| Performance: TDP, GPU clock, frame limit, refresh, scaling filter, FSR | Power profile, CPU and GPU limits, CPU cores, Frame rate cap, Refresh, Resolution + Scaling (FSR) + Sharpen. Per game, runner or All. |
| Performance overlay | HUD (frame rate, frame times); the sysinfo tab on the second screen |
| Quick settings (Wi-Fi, Bluetooth, brightness, night mode) | The bar's tabs; the tone curve for night colours |
| Keyboard | The keyboard tab |
| Notifications | ClamShell's notifications (Steam's own toasts stay in its window) |
| Friends, chat | Steam's window: the dock tile's tray menu |
| Steam Input layouts | Keymaps per game with layers and gyro; Steam Input as in F |
| PowerTools, SimpleDeckyTDP | Covered |
| lsfg-vk plugin | Frame generation tile (area 2) |
| Kettle Game Settings, Armada Control | Settings tab plus area 2's tiles |
| SteamGridDB | The Library's `steamgriddb` metadata |
| ProtonDB badges | Not covered. x86 ratings say little about ARM64EC. |
| CSS Loader, Animation Changer | Not applicable (Steam's own look) |

Today's gap, found while reading the code: **ClamShell's per-game environment never reaches a Steam game.**
- `session/running.rs` builds the cap's variables and the Env tile's variables and sends them with `Launch`.
- `launch/steam.rs::ask_steam` passes them only to the `steam -applaunch` process, which hands the request to the running client and ends. A cold start of Steam gets only the displays.
- So the Frame rate cap's FIFO variables, `SDL_GAMECONTROLLER_IGNORE_DEVICES` and the Env tile do nothing for Steam games. Nothing is dimmed and nothing is logged. Phase S2 fixes this.

### 1.2 Recommendation

(Decided 2026-10-30, after the plan: Steam is installed at the first boot rather than by a button; ClamShell's own compatibility tool is dropped; Steam stays a visible program, opened from the Library and the dock, with Big Picture for what is Steam's. See 1.9.)

G:
- Steam is set up from the Device view.
- A launch hook gives each Steam game its settings.
- The settings tab is the per-game panel.
- The Library later shows downloads.

Steam's own Big Picture keeps the store, friends, downloads and its per-game Properties: the Proton choice, the Steam Input layout, the launch options.

### 1.3 Flows and mockups

Conventions:
- The top screen is drawn 96×27 characters (1920×1080) and the bottom 62×27 (1240×1080).
- `>` is the pad's focus (amber). `[art]` and the boxes are pictures, not frames.
- Text is grey; what is dimmed is said under each mockup.

**Flow S-1: set up Steam.**
1. The Steam folder in Games, before setup. The folder is dim and its details say why. A goes to the Device view's Steam row (M2).

```
M1  Library: Games, the Steam folder before Steam is set up (top screen)
+------------------------------------------------------------------------------------------------+
|                                                                                                |
|        Recent      Favourites      Games       Apps       Files       Tools                    |
|                                    -----                                                       |
|   < Games                                                                                      |
|                                                                                                |
|     [art]  SNES                      14            +--------------------------+                |
|     [art]  PlayStation                3            |                          |                |
|  >  [art]  Steam                                   |      Steam's picture     |                |
|     [art]  PSP                        2            |                          |                |
|     [art]  Arcade                     9            |                          |                |
|                                                    |                          |                |
|                                                    +--------------------------+                |
|                                                                                                |
|                                                     Steam                                      |
|                                                     Steam isn't set up.                        |
|                                                                                                |
|                                                     Set up Steam                   A           |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
+------------------------------------------------------------------------------------------------+
```

2. The Device view gets two rows, Steam and Android. A chosen row fills the right side, as calibrating the sticks already does.

```
M2  Device view: the Steam row chosen
+------------------------------------------------------------------------------------------------+
|                                                                                                |
|     Wi-Fi                              Home-5G        Steam                                    |
|     Time zone                      Europe/Oslo                                                 |
|     Password                               set        Valve's client for ARM64 Linux, put in   |
|     Remote login                           Off        your home folder. Valve publishes it     |
|     Sticks                           Calibrate        but doesn't support it.                  |
|     Gyro                             Calibrate                                                 |
|     Top screen refresh               Calibrate        Windows games run through Proton         |
|  >  Steam                           Not set up        (ARM64), which Steam fetches.            |
|     Android                         Not set up        About 600 MB before any game.            |
|                                                                                                |
|                                                       Set up Steam                             |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                  Close                                                         |
|                                                                                                |
+------------------------------------------------------------------------------------------------+
```

3. Setting up. Steps not reached yet are dim. Stop ends it; setup is resumable, since `steam-setup` moves each part into place whole.

```
M3  Device view: Steam being set up
+------------------------------------------------------------------------------------------------+
|                                                                                                |
|     Wi-Fi                              Home-5G        Setting up Steam                         |
|     Time zone                      Europe/Oslo                                                 |
|     Password                               set        Valve's runtime for the client    done   |
|     Remote login                           Off        The client                        41 %   |
|     Sticks                           Calibrate          88 of 214 MB                           |
|     Gyro                             Calibrate        The client's first run                   |
|     Top screen refresh               Calibrate        Links                                    |
|  >  Steam                      Setting up 41 %                                                 |
|     Android                         Not set up        The first run shows Steam's own update   |
|                                                       window on this screen; it closes by      |
|                                                       itself.                                  |
|                                                                                                |
|                                                       Stop                                     |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                  Close                                                         |
|                                                                                                |
+------------------------------------------------------------------------------------------------+
```

4. Set up. Open Steam starts it; its first start is Steam's sign-in in Big Picture. Update Steam runs `steam-setup --update`. It is dimmed while Steam runs, with the reason under it. "Newest" is read when the row opens; the manifest is one small download.

```
M4  Device view: Steam set up, a newer client published
+------------------------------------------------------------------------------------------------+
|                                                                                                |
|     Wi-Fi                              Home-5G        Steam                                    |
|     Time zone                      Europe/Oslo                                                 |
|     Password                               set        Client          beta 1730912345          |
|     Remote login                           Off        Newest          beta 1731200000          |
|     Sticks                           Calibrate                                                 |
|     Gyro                             Calibrate        Open Steam                               |
|     Top screen refresh               Calibrate        Update Steam                             |
|  >  Steam                         Update ready          Quit Steam first: it is running        |
|     Android                         Not set up                                                 |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                  Close                                                         |
|                                                                                                |
+------------------------------------------------------------------------------------------------+
```

When the hook isn't in place (Steam put a real file where our link goes), the row reads "Settings don't reach games" in amber, and the right side says why.

**Flow S-2: play a Steam game with its settings.** This is the settings tab as it is, now true for Steam games. Settings that act at launch say "next start" while the game runs: Resolution as today, plus the Env tile and area 2's tiles.

```
M5  Settings tab for a Steam game while it runs (bottom screen)
+--------------------------------------------------------------+
|  set  dock  kbd  pad  sys   vol  bri   wifi  bt  bat  12:41  |
|                                                              |
|  All    Steam    Hades                       applies now     |
|                                                              |
|                                                              |
|    (pwr)       (fps)        (hz)       (hud)       (col)     |
|                                                              |
|   Balanced     60 fps     Hz as is    HUD off     Default    |
|                                                              |
|                                                              |
|    (res)       (scl)       (size)       (aa)       (shp)     |
|                                                              |
|    Native     Bilinear    Default      AA off   Sharpen off  |
|  next start                                                  |
|                                                              |
|    (pad)       (lit)       (mse)       (2nd)        (bg)     |
|                                                              |
|   Default    Lights off  Mouse off    Off idle     Pauses    |
|                                                              |
|                                                              |
|    (env)       (fan)       (cpu)       (gpu)       (rmb)     |
|                                                              |
|    1 set      Fan auto    CPU auto    GPU auto  Rumble full  |
|  next start                                                  |
|                                                              |
|                                                              |
|                                                              |
+--------------------------------------------------------------+
```

If the hook can't get a game's settings, the game still starts, and ClamShell says so. The hook writes one line to the control pipe, which becomes a notice:

```
M6  A Steam game started without its settings (a notice)
+--------------------------------------------------------------+
|  set  dock  kbd  pad  sys   vol  bri   wifi  bt  bat  12:41  |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|    Hades started without ClamShell's settings:               |
|    its settings couldn't be read (see the log).              |
|                                                              |
|                                                              |
|                                                              |
+--------------------------------------------------------------+
```

**Flow S-3 (phase S3): a game installing in Steam.** The row is dimmed with its progress and can't be played. A opens it in Steam (`steam://nav/games/details/<id>`).

```
M7  Library: the Steam folder with a game installing (phase S3)
+------------------------------------------------------------------------------------------------+
|                                                                                                |
|        Recent      Favourites      Games       Apps       Files       Tools                    |
|                                    -----                                                       |
|   < Steam                                                                                      |
|                                                                                                |
|     [art]  Hades                    12 h            +--------------------------+               |
|  >  [art]  Hades II           installing 43 %       |                          |               |
|     [art]  Celeste                   3 h            |     Hades II box art     |               |
|     [art]  Dave the Diver                           |                          |               |
|                                                     |                          |               |
|                                                     |                          |               |
|                                                     +--------------------------+               |
|                                                                                                |
|                                                      Hades II                                  |
|                                                      Installing in Steam: 5.2 of 12.1 GB       |
|                                                                                                |
|                                                      Open in Steam                 A           |
|                                                      Favourite                     Y           |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
+------------------------------------------------------------------------------------------------+
```

### 1.4 The launch hook

Every game Steam starts goes through `steamrtarm64/steam-launch-wrapper`. That is the link `steam-setup --links` already makes, after Thorch: Proton exits with 127 without it. The hook works like this:

- **The link.** `steam-setup --links` points it at `/usr/lib/clamshell/steam-launch-wrapper` instead of `../linuxarm64/steam-launch-wrapper`. The link is remade at every Steam start. Steam runs with `-nobootstrapupdate`, so it doesn't update itself mid-session and can't remove the link meanwhile.
- **The hook script.** `/usr/lib/clamshell/steam-launch-wrapper`, in the `clamshell` package and kept in `clamshell/steam/`:
  - It takes the app id from its arguments (`AppId=<id>`, as `steam_launchers` reads it), else from `SteamAppId`.
  - It runs `clamshell --launch-env steam:<id>` and exports the `KEY=value` lines that prints.
  - It execs Valve's `~/.local/share/Steam/linuxarm64/steam-launch-wrapper "$@"`.
  - On failure it runs the game as it is, logs through `logger -t clamshell-steam`, and sends `notice launch-env <id> <reason>` through the control pipe (M6).
- **`clamshell --launch-env <entry id>`** is new in `src/cli.rs` and `src/commands.rs`:
  - It needs no running ClamShell. It reads `settings/*.json` through `Store` and library.json's runners and sources (no scan), finds the entry's runner level (the Steam source that would take it, else none), and prints the variables.
  - It uses the same function the state uses for its own launches, so there is one place that computes a launch's environment.
- **What the game gets, later ones overriding earlier ones:** the settings' variables (area 2, the cap's FIFO), then `SDL_GAMECONTROLLER_IGNORE_DEVICES`, then the Env tile's variables for All, the runner and the game.
  - The user's own Steam launch options (`VAR=x %command%`) come after the hook in Steam's command, so they win over all of it.
  - The order is ClamShell's levels, then Steam's launch options. `docs/runners.md` says so.
- **`ask_steam` simplifies.** It stops passing the environment to `steam -applaunch`, which never reached the game.

The other way, considered: write `<hook> %command%` into each game's `LaunchOptions` in `userdata/<id>/config/localconfig.vdf` before Steam starts (Steam rewrites that file while it runs). Games installed during a session would miss the hook until Steam's next start, and the user's own launch options get edited. Kept only as the fallback if the device shows Steam doesn't call the link for every game.

### 1.5 Code

- `packages/thor-session/root/usr/bin/steam-setup`:
  - `--progress`: machine-readable lines (`step <n>/<total> <name>`, `bytes <done> <total>`).
  - `--status`: whether it is installed, the installed version, the branch, and whether the hook link is ours.
  - `--newest`: the manifest's version.
  - The hook link.
  - It stays usable by hand.
- `packages/thor-session/root/usr/bin/steam`: when Steam isn't set up, it prints "Steam isn't set up: Tools, Device, Steam" and exits 1, without opening a terminal. ClamShell reports that through the Steam runner's problem.
- `crates/state/src/device_view/`:
  - New `steam.rs`. The machine: `Unread → NotSetUp | Ready{version, newest, hook_ok} | Failed(reason)`; `NotSetUp --SetUp--> SettingUp{step, of, bytes}`; `SettingUp --Done--> Ready`; `--Failed(e)--> Failed`; `--Stop--> NotSetUp`; `Ready --Update (Steam not running)--> Updating --Done--> Ready`.
  - The Device view's `Id` gets the two rows. Opening the row reads the status.
  - `docs/state-machines.md` gets the section, with a test per transition, including Update refused while `desktop:steam` runs, and a thread restarted during setup.
- `crates/system/src/install/` (new): a thread that runs a setup helper as a child, parses its progress lines into events, and reports exit and stderr. Steam's helper now, Android's in area 3. One implementation for both.
- `crates/views/src/device.rs`: the two rows and their right-side panes, drawn with the existing row and text widgets.
- The Library's empty and problem state:
  - A runner whose setup isn't done shows like a system's empty folder: dimmed, with a reason and an action.
  - The Steam runner (`type: steam`) knows its setup. A runner from library.json names one with a new field, `"setup": "android"`, which is a Device view row.
  - `crates/library/src/config.rs`, `crates/state/src/library_view/`, `crates/views/src/library_view.rs`.
- Phase S3:
  - `crates/library/src/steam.rs::manifest` returns an install state (StateFlags, `BytesDownloaded` / `BytesToDownload`) instead of leaving partly installed games out.
  - The system's Steam library reader watches `steamapps/*.acf` with inotify, already used elsewhere.
  - An installing entry gets no Play; its action is "Open in Steam".
- `src/cli.rs`, `src/commands.rs`: `--launch-env`.
- `crates/state/src/settings/`: the launch environment (area 2, 3.4) moves here from `session/running.rs::cap_env`, as `launch_env(values, &mut env)`.

### 1.6 Packaging

| What | From | Where | Updated |
|---|---|---|---|
| Steam client | Valve's CDN, `steam-setup` (as now) | `~/.local/share/Steam` | Device view "Update Steam" (`steam-setup --update`), or maintenance |
| Valve's runtime for the client | Valve, `steam-setup` | same | with the client |
| Valve's Proton ARM64 | Steam fetches it | Steam's library | Steam |
| The hook script | our `clamshell` package | `/usr/lib/clamshell/steam-launch-wrapper` | pacman |

### 1.7 What only the device can answer

1. Whether Steam runs `steamrtarm64/steam-launch-wrapper` for every launch: Proton ARM64, native arm64 Linux games, and a Linux x86 game Steam would run through FEX. If not, the `localconfig.vdf` fallback (1.4).
2. Whether variables set before Valve's wrapper reach the game through pressure-vessel, if Proton ARM64 runs in the Steam Linux Runtime container. Try `FEX_*`, `VK_DRIVER_FILES` and `MESA_VK_WSI_PRESENT_MODE`.
3. Steam's in-game overlay over an ARM64EC game, and Guide (Home+Back, already in the gameplan) opening it.
4. With Xbox configuration support turned on in Steam:
   - Steam lists one controller (ClamShell's player 1 pad).
   - The game sees only Steam's virtual pad.
   - Community layouts apply.
   - Steam's desktop layout doesn't act on the pad outside games.
5. The first run's update window under ClamShell (`steam -exitsteam` with only the displays).
6. Big Picture's Quick Access menu outside gamescope: what it shows, and whether #13544's freeze happens here too.

### 1.8 Phases

- **S1. Installed at the first boot, shown and updated in the Device view.** No terminal. The Library's Steam folder says what is missing while it isn't done (offline at the first boot, or failed); the Device view's row retries it.
- **S2. The launch hook.** Steam games get every ClamShell setting that acts at launch, and the Env tile. Today's silent gap closes. On its own it already makes the settings tab the Steam games' panel. Area 2's Steam side depends on it.
- **S3. Games installing in Steam shown in the Library.** Optional: worth it if the device shows that people start downloads in Steam and come back to the Library.
- **S4. Device checks 3, 4 and 6.** Their results go into `docs/runners.md` (Steam Input and the overlay), with no code expected.

### 1.9 Decided after the plan (2026-10-30)

- **Steam at the first boot, not in the image.** Distributions ship only Steam's bootstrapper; the client and its runtime come from Valve on the user's machine, and nothing found allows us to redistribute them in a published image (only Valve preinstalls the whole client, on SteamOS). A baked copy would also be replaced by the client's own update within minutes. So `thor-firstboot` (or a unit after it, once online) runs `steam-setup` as the user; the Device view shows its progress, failure and retry, and updates it later (M2 to M4 become status and retry rather than a first "Set up" button). Valve's install agreement is still shown at Steam's first start, for the user to accept.
- **ClamShell's own compatibility tool, dropped.** Considered: a "ClamShell" tool in Steam's documented `compatibilitytools.d` interface running the Proton chosen in ClamShell, which would bring the Proton choice into ClamShell's settings. It needs each game set to it in Steam once, and its manifest's runtime has to match the chosen Proton's, unproven on the ARM client. The launch hook (1.4) stays, and the Proton choice stays in Steam's Properties.
- **Steam stays a program of its own, out of the way.** It opens from the Library and the dock when the user asks for it; Big Picture keeps the store, downloads, friends, Steam Input and the Proton choice. Launching an installed game shows only the game (`steam -gamepadui -silent -applaunch`, as now): no part of Steam appears unless it needs an answer (a cloud-save conflict, a component install, the cold start's update progress). Any other Steam window raised by a launch is a bug. ClamShell adds the setup, the per-game settings reaching the game, and the Library's entries.
- **S2 is tested before the device.** The desktop's x86 Steam runs games through `steam-launch-wrapper` and Valve's runtime container as the ARM client does, so nested ClamShell checks that Steam calls the hook with the app id for every launch, that `clamshell --launch-env` resolves a Steam game's settings, and that the variables reach the game inside the container. Left for the device: the ARM path (`steamrtarm64/`), FEX inside ARM64 Proton reading the preset, and `VK_DRIVER_FILES` reaching Turnip.
- **x86 emulation is applied by `FEX_APP_CONFIG`** (Armada's way, its wrapper: "the only knob every compat tool's FEX reads"), not by individual `FEX_*` variables: the two presets ship as fixed JSON files and are copied to `~/.cache/clamshell/fex/`, since Valve's runtime container replaces `/usr` and wouldn't show them from `/usr/share`. Proton's own sets nothing. The tile is dimmed for entries that don't run x86 code.

---

## 2. PC-gaming tuning

### 2.1 Options

**Proton version per game.**
- (a) A ClamShell tile writing Steam's `CompatToolMapping` (config.vdf) at Steam's start, or through `SteamClient` over CEF.
- (b) Leave the choice in Steam's own Properties > Compatibility page, which is already per game, in Big Picture, by controller. Make more versions appear there by packaging them.
- (a) duplicates a control Steam already has. The two would fight: ClamShell writes the mapping while stopped, and the user changes it in Steam. And (a) applies only at Steam's next start, or needs CEF, rejected in 1.1 E.
- Recommendation: (b). Package Proton-CachyOS ARM64, whose `PROTON_USE_OPTISCALER` `docs/picture.md` relies on. A second community Proton (GE-Proton ARM64) only if a game needs it.

**FEX per game.**
- (a) Armada's three presets.
- (b) Kettle's seven individual options.
- (c) FEX versions.
- (c) isn't separable: FEX comes inside Proton ARM64 (`libarm64ecfex.dll`), so picking a FEX version means picking a Proton.
- (b) is for people who already know FEX; the Env tile takes `FEX_SMCCHECKS=full` and the like for them.
- Recommendation: (a), as one choice setting. The values:
  - Proton's own: no variables.
  - Fast: TSO off, half barriers off, x87 reduced, multiblock.
  - Safe: TSO, vector and memcpy TSO on, x87 full precision, multiblock off.
- They are applied as `FEX_*` variables, Kettle's way. Where those aren't read (Armada's comment), the hook writes a `FEX_APP_CONFIG` file under `~/.cache`, Armada's way, which pressure-vessel shares.
- Thunks: dropped. They matter only to the system FEX, which runs x86 Linux games on an x86 root filesystem we don't ship.

**Turnip per game.**
- (a) Armada's set: Stable, Devel, WinNative, plus drivers the user drops into their home.
- (b) Two: the system's (Arch Linux ARM's `vulkan-freedreno`) and ours from Mesa main.
- User-supplied drivers can't be told apart from an Android build until something fails (Armada has to check ELF and libc for that). WinNative is one fork's patch set.
- Recommendation: (b), a choice setting "Vulkan driver": System or Newest, through `VK_DRIVER_FILES`.
- It applies to every Vulkan program ClamShell launches (emulators too), not only Proton games. That is why it is a general setting.
- Notes:
  - Proton ARM64 games use the native aarch64 loader, so the native driver applies (Kettle's PKGBUILD says so).
  - x86 Linux games under the system FEX would load an x86 Turnip, which we don't ship.
  - vkd3d-proton's pipeline cache is thrown away when the driver changes. The hook can give each driver its own `VKD3D_SHADER_CACHE_PATH` under `STEAM_COMPAT_SHADER_PATH`, Armada's way, if the device shows stutter from it.

**Frame generation.**
- (a) lsfg-vk (MIT). It needs `Lossless.dll` from Lossless Scaling, which the user buys on Steam (app 993090). Multipliers 2x to 4x.
- (b) kettle-framegen (BSD-3). It is its own interpolation, tuned on the Adreno 740, 2x only, with no purchase.
- (c) In the compositor, which `docs/picture.md` left open. It would have no motion vectors, and it would cost the compositor's thread GPU time it has to spare for every window.
- Both (a) and (b) are implicit Vulkan layers, loaded by the native loader, so they reach ARM64EC games too.
- Recommendation: one "Frame generation" setting, Off or 2x, implemented by one layer. Prefer (b): nothing to buy and nothing found in the user's Steam. Try it on the device first, then (a) if (b) falls short. If (a) is chosen, the 2x row dims "needs Lossless Scaling (bought on Steam)" until the DLL is found.
- Interactions:
  - The compositor's Frame rate cap and HUD count frames shown. With frame generation the game draws half of them. The menu says to pair it with a cap of half the screen's rate (M11).
  - Frame generation dims when picture.md's In game frame generation (OptiScaler) is on.

**Upscalers through OptiScaler.** Already `docs/picture.md` layer 1 (Proton-CachyOS `PROTON_USE_OPTISCALER=1`, its phase 2 on the device). New fact: Kettle had to build OptiScaler for ARM64EC (`packages/optiscaler-arm64ec`, with SGSR2 and ASR patches); an x86-64 build would run emulated. picture.md's phase 2 should test with an ARM64EC build, packaged as `optiscaler-arm64ec` from Kettle's recipe (llvm-mingw).

**Dropped:**
- Kettle's game database and community ratings: a server and moderation.
- Auto TDP: the power profiles and limits already exist.
- Per-game Lumen and Nanite edits to UE5's Engine.ini: game-specific file editing.
- Armada's core affinity, nice values and scheduler: CPU cores already exists. The one useful bit, `WINE_CPU_TOPOLOGY` when the big cores are off, could join the launch environment if the device shows Wine counting cores wrongly.

### 2.2 Recommendation

Three choice settings, through the launch environment, at every level:

| Setting (file key) | Values | Variables |
|---|---|---|
| x86 emulation (`x86`) | Proton's own (0), Fast (1), Safe (2) | `FEX_TSOENABLED`, `FEX_VECTORTSOENABLED`, `FEX_MEMCPYSETTSOENABLED`, `FEX_HALFBARRIERTSOENABLED`, `FEX_X87REDUCEDPRECISION`, `FEX_MULTIBLOCK` (Armada's two presets) |
| Vulkan driver (`vkdriver`) | System (0), Newest (1) | `VK_DRIVER_FILES=/usr/lib/turnip-devel/share/vulkan/icd.d/freedreno_icd.aarch64.json` |
| Frame generation (`framegen`) | Off (0), 2x (2) | the layer's switch (`KETTLE_FG=1`, or lsfg-vk's) |

Proton stays in Steam's Properties, with Proton-CachyOS ARM64 packaged.

### 2.3 Flows and mockups

The grid grows by one row: 21 tiles become 24, and four rows still show at once while it scrolls. The new tiles come after Rumble. Reset and CPU cores end the row, as now, so Reset shows with the others on a program's tab.

```
M8  The settings tab scrolled to its last row (the same game)
+--------------------------------------------------------------+
|  set  dock  kbd  pad  sys   vol  bri   wifi  bt  bat  12:41  |
|                                                              |
|  All    Steam    Hades                       applies now     |
|                                                              |
|                                                              |
|    (res)       (scl)       (size)       (aa)       (shp)     |
|                                                              |
|    Native     Bilinear    Default      AA off   Sharpen off  |
|  next start                                                  |
|                                                              |
|    (pad)       (lit)       (mse)       (2nd)        (bg)     |
|                                                              |
|   Default    Lights off  Mouse off    Off idle     Pauses    |
|                                                              |
|                                                              |
|    (env)       (fan)       (cpu)       (gpu)       (rmb)     |
|                                                              |
|    1 set      Fan auto    CPU auto    GPU auto  Rumble full  |
|  next start                                                  |
|                                                              |
|    (gen)       (drv)       (x86)      (reset)     (cores)    |
|                                                              |
| Framegen 2x Mesa newest  x86 Proton    Reset     All cores   |
|  next start  next start  next start                          |
|                                                              |
|                                                              |
|                                                              |
+--------------------------------------------------------------+
```

A tap steps through the values. A long press opens the menu, which is the existing tile menu. The rows gain an optional dim second line saying what the value does. That is an addition to the one shared menu widget, which every tile then may use, not a menu of its own.

```
M9  The x86 emulation tile's menu (a long press)
+--------------------------------------------------------------+
|  set  dock  kbd  pad  sys   vol  bri   wifi  bt  bat  12:41  |
|                                                              |
|   x86 emulation                                              |
|                                                              |
| > Proton's own                                  in force     |
|   FEX as Proton sets it up.                                  |
|                                                              |
|   Fast                                                       |
|   No x86 memory ordering. Faster; many games                 |
|   crash or hang with it.                                     |
|                                                              |
|   Safe                                                       |
|   Strict memory ordering, full x87 precision.                |
|   Slower; for crashes and glitches.                          |
|                                                              |
|   Reset to Steam's: Proton's own                             |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|   Applies at the game's next start.                          |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
+--------------------------------------------------------------+
```

On a desktop without `turnip-devel`, Newest is dimmed with "not installed", and the tile too when it is chosen. The versions are read from pacman's local database.

```
M10 The Vulkan driver tile's menu
+--------------------------------------------------------------+
|  set  dock  kbd  pad  sys   vol  bri   wifi  bt  bat  12:41  |
|                                                              |
|   Vulkan driver                                              |
|                                                              |
|   System                Mesa 26.2.5          in force        |
|   Turnip as the system ships it.                             |
|                                                              |
| > Newest                Mesa main 2026-10-13                 |
|   Turnip's newest fixes; may bring new faults.               |
|                                                              |
|   Reset to All's: System                                     |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|   For programs that draw with Vulkan: Proton's               |
|   games, most emulators. Applies at the next start.          |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
+--------------------------------------------------------------+
```

2x is dimmed with its reason when the layer isn't installed, or when In game frame generation is on ("the game generates frames").

```
M11 The Frame generation tile's menu
+--------------------------------------------------------------+
|  set  dock  kbd  pad  sys   vol  bri   wifi  bt  bat  12:41  |
|                                                              |
|   Frame generation                                           |
|                                                              |
| > Off                                           in force     |
|                                                              |
|   2x                                                         |
|   A frame made between each two the game draws.              |
|   Smoother, about a frame later; costs GPU time.             |
|   Pair it with a Frame rate cap of half the                  |
|   screen's rate, so frames come evenly.                      |
|                                                              |
|   Reset to All's: Off                                        |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|                                                              |
|   Applies at the next start.                                 |
|                                                              |
|                                                              |
|                                                              |
+--------------------------------------------------------------+
```

For Android, the three new tiles are dimmed "no effect", as the Env tile already is. That comes from `Setting::through_launch` and `Panel::handed_over`; no new rule.

```
M12 An Android app's settings, scrolled to the last row
+--------------------------------------------------------------+
|  set  dock  kbd  pad  sys   vol  bri   wifi  bt  bat  12:41  |
|                                                              |
|  All    Android    Clash Royale              applies now     |
|                                                              |
|                                                              |
|    (res)       (scl)       (size)       (aa)       (shp)     |
|                                                              |
|    Native     Bilinear    Default      AA off   Sharpen off  |
|  next start                                                  |
|                                                              |
|    (pad)       (lit)       (mse)       (2nd)        (bg)     |
|                                                              |
|   Default    Lights off  Mouse off    Off idle     Pauses    |
|                                                              |
|                                                              |
|    (env)       (fan)       (cpu)       (gpu)       (rmb)     |
|                                                              |
|  no effect    Fan auto    CPU auto    GPU auto  Rumble full  |
|                                                              |
|                                                              |
|    (gen)       (drv)       (x86)      (reset)     (cores)    |
|                                                              |
| No framegen Mesa system  x86 Proton    Reset     All cores   |
|  no effect   no effect   no effect                           |
|                                                              |
|                                                              |
|                                                              |
+--------------------------------------------------------------+
```

Choosing a game's Proton has no ClamShell screen: it is Steam's Properties > Compatibility, in Big Picture, where Proton-CachyOS ARM64 appears once its package is installed.

x86 emulation is not dimmed for native programs. ClamShell can't know whether an item runs x86 code (a Steam game may be native arm64), and the variables do nothing where FEX isn't. Dimming it where unsure would be a guess. Its menu text says what it is for.

### 2.4 Code and data

- `crates/state/src/settings/mod.rs`:
  - `Setting::X86`, `Setting::VulkanDriver`, `Setting::FrameGen` as `choices!` enums.
  - `COUNT` 31 → 34, and names in `names!`. No migration: the files just gain keys.
  - `through_launch()` adds the three.
  - `launch_env(values, &mut env)` replaces `session/running.rs::cap_env`. The cap's variables come first; the order is pinned by a test, since the user's Env tile must win.
- `crates/state/src/settings/panel.rs`: `GRID` gains the three tiles.
  - A tile or value whose component is missing is dimmed "not installed".
  - Facts: `Event::Installed { turnip_devel: Option<version>, framegen: bool, mesa: version }`, read by the system crate at start. `Sys` keeps them.
  - The menu's second lines come from a `Choice` description table beside the labels.
- `crates/views/src/settings.rs`, `icons.rs`: three line icons.
- `crates/system/src/library/` or `paths.rs`: the component paths, as constants of the packages that install them.
- `src/commands.rs` (`--launch-env`) calls `launch_env`, so Steam games and ClamShell's launches get the same variables.
- `docs/state-machines.md`, Settings: the new settings, the env order test, the dimming rules.
- `docs/picture.md`: layer 3 changes from "left open" to the Frame generation setting as a launch-time layer, and the HUD wording "60 fps, 30 rendered" is dropped. The compositor can't tell generated frames from a layer's.

### 2.5 Packaging

| Package (our repo) | Built from | Installs | Updated |
|---|---|---|---|
| `turnip-devel` | Mesa main at a pinned commit, Kettle's `kettle-turnip` PKGBUILD as the model: plain meson, `-Dvulkan-drivers=freedreno -Dgallium-drivers=`, `--libdir lib/turnip-devel`, `--datadir lib/turnip-devel/share`. Built in an aarch64 chroot or by cross meson, which is new for our build. | `/usr/lib/turnip-devel/` with its ICD JSON outside the loader's search path, so no program sees two GPUs; about 15 MB | We bump the commit when Mesa main has a Turnip fix worth having; pacman -Syu |
| `kettle-framegen` (or `lsfg-vk`) | Upstream at a pinned commit (C, glslang). lsfg-vk: CMake; the DLL is the user's. | The implicit layer's library and manifest under `/usr/share/vulkan/implicit_layer.d`, enabled only by its variable | Bumped by us |
| `proton-cachyos-arm64` | The CachyOS release asset `proton-cachyos-<ver>-arm64.tar.xz`, checked against its `.sha512sum` (Armada's `packages/proton-cachyos/build.sh`) | `/usr/share/steam/compatibilitytools.d/proton-cachyos-arm64/`, under a fixed internal name so that games set to it survive updates (Armada's `set-tool-name.py`); about 1 GB unpacked | Bumped by us |
| `optiscaler-arm64ec` | Kettle's recipe (llvm-mingw); picture.md phase 2 | Where Proton-CachyOS looks for OptiScaler | Bumped by us |

- The `thor` metapackage depends on `turnip-devel` and the frame generation layer, which are small.
- `proton-cachyos-arm64` stays optional until the device shows a game that needs it, or OptiScaler works. Its size on an SD card is the reason.
- Arch Linux ARM supplies the system Mesa (`vulkan-freedreno`).
- Read Armada's `patch-proton-cachyos-dxvk-probe.py` before packaging, to learn what it works around on ARM.

### 2.6 What only the device can answer

1. Whether `FEX_*` variables are read by FEX inside Proton ARM64 (Kettle relies on it; Armada says only `FEX_APP_CONFIG` is). Test with a TSO-sensitive game, comparing `FEX_TSOENABLED=0` with 1.
2. Whether `VK_DRIVER_FILES` pointing at `/usr/lib/turnip-devel` reaches a Proton ARM64 game through pressure-vessel (Armada does it). `DXVK_HUD=devinfo` shows the driver's version.
3. Whether the frame generation layer works on the swapchain of Wine's Wayland driver (our default) and of X11, its GPU cost at 1080p, the latency it adds, and how it paces against the compositor's FIFO cap.
4. Whether Proton-CachyOS ARM64's toolmanifest (`require_tool_appid`) is satisfied by what Valve's ARM client fetches. Community tooling removes it (PB-OS's `Proton-ARM-Easy-Manager`).
5. Whether Turnip from Mesa main is stable on the A740 with the games we test.

### 2.7 Phases

- **P1. `launch_env` and x86 emulation.** No package. Delivers FEX presets for Steam games (after S2) and for everything ClamShell launches.
- **P2. Vulkan driver and `turnip-devel`.** Per-game newer Turnip, for emulators at once and for Steam games after S2.
- **P3. `proton-cachyos-arm64`.** A second Proton in Steam's picker. It enables picture.md's OptiScaler test.
- **P4. Frame generation.** Try kettle-framegen against lsfg-vk on the device, package one, add the tile.
- **P5. picture.md phase 2 with `optiscaler-arm64ec`.** Its In game tiles, if it works.

---

## 3. Android

### 3.1 Options

**A. Waydroid, as now.**
- LineageOS 20 (Android 13), one container, an app per window, the runner and Wayland filter already done (`docs/android.md`).
- On the Thor it isn't installed: `waydroid` is an optdepend, and setup is root commands typed by hand.
- Its GPU on the Adreno 740 is Waydroid's own Android Mesa. Armada overlays its own Mesa only for SM8750, which suggests the image's works on SM8550.

**B. Lepton (Valve).**
- An Android 11 image that Steam downloads (app 3029110, installable by `steam://install/3029110` on the generic ARM client, as Armada does). Valve's launcher (MIT) runs it in rootless podman, a container per app, drawn as a window. Its hardware layer is derived from Waydroid's.
- It needs:
  - Steam running: apps are Steam shortcuts with Lepton as their compatibility tool. ClamShell's Library reads only appmanifests; shortcuts would need `shortcuts.vdf` (binary VDF) and adding them over CEF (Kettle's `kettle-android-games`).
  - podman, passt and crun.
  - An Android build of Mesa: Kettle uses Valve's deckard binaries, Armada builds `mesa-android` with the NDK.
  - Patches each project carries: the 2D window, pads, split APKs, the init keychord.
- Android 11 is older than Waydroid's 13. It has no Google apps.
- Gains: Valve maintains it, and it runs Android games Steam might sell for the Frame.
- Verdict: no for now. Revisit if Valve ships it outside Steam, if Steam sells Android games people want, or if Waydroid's GPU fails on the A740.

**C. Google Play.** Google's apps (Play Services, the Play Store) are licensed only to manufacturers under GMS agreements, which need certified devices. Nobody else may redistribute them, so **our repository can never ship them, nor an image containing them.** The ways people get them anyway:
- (a) **Waydroid's GAPPS image** (`waydroid init -s GAPPS`): LineageOS with MindTheGapps, downloaded from Waydroid's OTA servers on the user's device after the user chooses it. Google's own page for custom ROMs, `google.com/android/uncertified`, registers the device's Android ID so that sign-in works.
  - We distribute nothing.
  - The images themselves are distributed by Waydroid without a licence from Google: tolerated, not licensed. The plan says so to the user (M13).
  - Games that check Play Integrity strongly may still refuse.
- (b) PB-OS's way: fetch MindTheGapps on the device, patch `services.jar` with smali, and present the device as a certified Pixel 5. Presenting false device identity to Google is not something to ship. It also breaks with each Lepton image. No.
- (c) Aurora Store (GPL), anonymous Play downloads (Armada's `armada-aurora`; Kettle's apkeep). The terms of the anonymous accounts are grey, and apps needing Play Services still fail. Not shipped. A user can install Aurora from F-Droid.
- (d) microG. It needs signature spoofing and an image of our own. No.

### 3.2 Recommendation

Waydroid, set up from the Device view, with one choice made at setup:
- **Android:** the vanilla image, with F-Droid's APK shipped in our package (GPL-3) and installed at setup.
- **Android with Google Play:** the GAPPS image Waydroid downloads, then a registration step (M15).

Also:
- The players' pads reach Android, so ClamShell's keymaps work for Android games.
- The Android runner's place becomes its own folder in Apps, with the empty-state action (M17).

### 3.3 Flows and mockups

1. The Android folder before setup:

```
M17 Library: Apps, the Android folder before Android is set up
+------------------------------------------------------------------------------------------------+
|                                                                                                |
|        Recent      Favourites      Games       Apps       Files       Tools                    |
|                                                 ----                                           |
|   < Apps                                                                                       |
|                                                                                                |
|     [art]  Firefox                                 +--------------------------+                |
|  >  [art]  Android                                 |                          |                |
|     [art]  Maintenance                             |    Android's picture     |                |
|                                                    |                          |                |
|                                                    |                          |                |
|                                                    |                          |                |
|                                                    +--------------------------+                |
|                                                                                                |
|                                                     Android                                    |
|                                                     Android isn't set up.                      |
|                                                                                                |
|                                                     Set up Android                 A           |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
+------------------------------------------------------------------------------------------------+
```

2. The choice:

```
M13 Device view: the Android row chosen, before setup
+------------------------------------------------------------------------------------------------+
|                                                                                                |
|     Wi-Fi                              Home-5G        Android                                  |
|     Time zone                      Europe/Oslo                                                 |
|     Password                               set        Android apps through Waydroid: Android   |
|     Remote login                           Off        13 (LineageOS 20) in a container on      |
|     Sticks                           Calibrate        this system. About 1 GB.                 |
|     Gyro                             Calibrate                                                 |
|     Top screen refresh               Calibrate        Set up Android                           |
|     Steam                                Ready          With F-Droid, a store of free apps.    |
|  >  Android                         Not set up                                                 |
|                                                       Set up Android with Google Play          |
|                                                         Google's apps come from Waydroid's     |
|                                                         servers, not ours. Google asks you     |
|                                                         to register the device. Games that     |
|                                                         check the device's integrity may       |
|                                                         still refuse to run.                   |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                  Close                                                         |
|                                                                                                |
+------------------------------------------------------------------------------------------------+
```

3. Setting up. The root part runs through a pkexec helper that polkit allows for the active local user without a prompt, as `thor-account` is allowed (`50-thor.rules`).

```
M14 Device view: Android being set up
+------------------------------------------------------------------------------------------------+
|                                                                                                |
|     Wi-Fi                              Home-5G        Setting up Android                       |
|     Time zone                      Europe/Oslo                                                 |
|     Password                               set        Android's images (Waydroid)        63 %  |
|     Remote login                           Off          410 of 652 MB                          |
|     Sticks                           Calibrate        Android's first start                    |
|     Gyro                             Calibrate        F-Droid                                  |
|     Top screen refresh               Calibrate                                                 |
|     Steam                                Ready        From Waydroid's servers.                 |
|  >  Android                    Setting up 63 %                                                 |
|                                                       Stop                                     |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                  Close                                                         |
|                                                                                                |
+------------------------------------------------------------------------------------------------+
```

4. With Google Play only, registration. The Android ID is read with `sqlite3` from GSF's database inside the container. "Copy the ID and open the page" puts it on the clipboard (the clipboard path exists) and launches Firefox at the page. Done records it; the row then reads Ready. ClamShell can't tell whether Google accepted it, so it doesn't pretend to.

```
M15 Device view: registering with Google (Google Play chosen)
+------------------------------------------------------------------------------------------------+
|                                                                                                |
|     Wi-Fi                              Home-5G        Register this device with Google         |
|     Time zone                      Europe/Oslo                                                 |
|     Password                               set        Google Play signs in only on devices     |
|     Remote login                           Off        registered with it. Once:                |
|     Sticks                           Calibrate                                                 |
|     Gyro                             Calibrate        Android ID   3911286438152040763         |
|     Top screen refresh               Calibrate                                                 |
|     Steam                                Ready        1. Copy the ID and open the page         |
|  >  Android                           Register           (google.com/android/uncertified,      |
|                                                          in Firefox)                           |
|                                                       2. Sign in, paste the ID, Register       |
|                                                       3. Wait a few minutes, then open         |
|                                                          the Play Store                        |
|                                                                                                |
|                                                       Copy the ID and open the page            |
|                                                       Done                                     |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                  Close                                                         |
|                                                                                                |
+------------------------------------------------------------------------------------------------+
```

5. Set up. Update Android runs `waydroid upgrade`, dimmed while an Android app runs. Switching between the vanilla and GAPPS images is not offered after setup until the device shows whether `/data` survives it (3.6).

```
M16 Device view: Android set up
+------------------------------------------------------------------------------------------------+
|                                                                                                |
|     Wi-Fi                              Home-5G        Android                                  |
|     Time zone                      Europe/Oslo                                                 |
|     Password                               set        Waydroid       1.5.4                     |
|     Remote login                           Off        Android        LineageOS 20, 2026-10-12  |
|     Sticks                           Calibrate        Google Play    on                        |
|     Gyro                             Calibrate                                                 |
|     Top screen refresh               Calibrate        Update Android                           |
|     Steam                                Ready          Quit the Android apps first            |
|  >  Android                              Ready        Open the Play Store                      |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                  Close                                                         |
|                                                                                                |
+------------------------------------------------------------------------------------------------+
```

6. Apps listed as now (desktop files `waydroid.<package>`), in their folder. An Android app's settings are M12.

```
M18 Library: the Android folder with apps
+------------------------------------------------------------------------------------------------+
|                                                                                                |
|        Recent      Favourites      Games       Apps       Files       Tools                    |
|                                                 ----                                           |
|   < Android                                                                                    |
|                                                                                                |
|     [art]  F-Droid                                  +--------------------------+               |
|  >  [art]  Clash Royale                3 h          |                          |               |
|     [art]  Play Store                               |   Clash Royale's icon    |               |
|     [art]  Stardew Valley              9 h          |                          |               |
|                                                     |                          |               |
|                                                     |                          |               |
|                                                     +--------------------------+               |
|                                                                                                |
|                                                      Clash Royale                              |
|                                                      Android                                   |
|                                                                                                |
|                                                      Play                          A           |
|                                                      Favourite                     Y           |
|                                                      Pin to the dock                           |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
|                                                                                                |
+------------------------------------------------------------------------------------------------+
```

### 3.4 Code

- A new package, `packages/thor-android`:
  - `/usr/lib/thor/thor-android`, the root helper run through pkexec. It does fixed operations only:
    - `init vanilla|gapps`: `waydroid init [-s GAPPS]`, with progress parsed from its download output.
    - Enable `waydroid-container`.
    - `upgrade`.
    - Write the adb key into the container (what `docs/android.md` does by hand with `lxc-attach`).
  - Its rule in `50-thor.rules`, in thor-system.
  - The user part, `clamshell-android --setup`: the properties (`persist.waydroid.suspend false`, `multi_windows true`), heads-up notifications off, F-Droid installed from `/usr/share/thor/android/F-Droid.apk`, and, for Google Play, reading the Android ID.
- `crates/state/src/device_view/android.rs`: the machine is `Unread → NotSetUp | Ready{play, versions} | Failed`; `NotSetUp --SetUp(play)--> SettingUp{step, bytes} --Done--> (play ? Register{id} : Ready)`; `Register --Done--> Ready`; `Ready --Update (no Android app running)--> Updating --> Ready`; `Stop` and helper failures.
- `crates/system/src/install/android.rs` on the same thread as Steam's setup (1.5).
- `crates/views/src/device.rs`: the row and panes.
- `library.json` (the starter):
  - An `android` place in Apps. The `waydroid.*` source goes there.
  - The Android runner gains `"setup": "android"`, which ties the folder's empty state to the Device view row.
  - `docs/runners.md` documents `setup`.
- The pads in the container (phase A2):
  - Check first whether Waydroid's container sees ClamShell's players' pads (`getevent -l` in `waydroid shell`).
  - If not, a root service in thor-android publishes the virtual pads' event nodes into the container, as Armada's `waydroid-input-setup` does for InputPlumber's. Only ClamShell's virtual pads, never a grabbed controller.
  - Android then reads keymapped input, and ClamShell's per-game keymaps apply to Android games.
- `docs/android.md`: the Setup section becomes what the helper does, plus Google Play.

### 3.5 Packaging

| What | From | Where | Updated |
|---|---|---|---|
| `waydroid` and its dependencies (lxc, python-gbinder, dnsmasq) | Arch Linux ARM if it builds them (check `pacman -Si waydroid` on the device), else ours from Arch's PKGBUILD | system | pacman |
| `android-tools` (adb) | Arch Linux ARM | system | pacman |
| `thor-android` (helper, rule, F-Droid APK pinned by version and SHA-256) | our repo | `/usr/lib/thor`, `/usr/share/thor/android` | pacman. F-Droid then updates itself inside Android. |
| Android's images, vanilla or GAPPS | Waydroid's OTA servers, fetched by `waydroid init` on the user's device | `/var/lib/waydroid/images` | Device view "Update Android" (`waydroid upgrade`) |
| Google's apps | inside Waydroid's GAPPS image only; never in our repo | | with the image |

The `thor` metapackage depends on `thor-android` and `waydroid`, so that setup needs no pacman step. The images, the large part, come only when the user chooses to set Android up.

### 3.6 What only the device can answer

1. Whether Waydroid's image draws on the Adreno 740 with its own Mesa (Armada suggests yes for SM8550), and at what frame rate in a 3D game.
2. Whether arm64 GAPPS images are published for LineageOS 20 on Waydroid's OTA, and whether the uncertified-device registration lets Play Store sign in.
3. Whether `waydroid init -f -s GAPPS` over a vanilla install keeps `~/.local/share/waydroid/data` usable. If yes, offer switching from M16.
4. Whether the virtual pads are seen inside the container without help.
5. The network: dnsmasq and Waydroid's nftables rules on our kernel (the gameplan notes they are in ROCKNIX's config). There is no firewall in our image to open.
6. Which popular Android games refuse to run for Play Integrity reasons.

### 3.7 Phases

- **A1. `thor-android`, the Device view row, vanilla Android with F-Droid.** Anyone can have Android apps with no terminal.
- **A2. The pads in the container.** Android games playable with the controller and ClamShell's keymaps.
- **A3. Google Play through Waydroid's GAPPS image, with the registration pane.** The Play Store for those who choose it.
- **A4. Update Android.** Also usable from maintenance.

---

## 4. Interactions

- **One settings model.** The new settings sit beside the picture, power and input settings: same levels, same tile, same menu, same dimming. Every setting acting at launch goes through `settings::launch_env`, used by ClamShell's own launches and by `clamshell --launch-env` for Steam's hook.
- **Steam's launch options against ClamShell's runner.** ClamShell's levels (All, then the Steam runner, then the game, then the Env tile) come first. The user's Steam launch options come after the hook in Steam's command, so they win. The Proton choice is Steam's; FEX, the driver and frame generation apply whichever Proton runs.
- **Launchers' games** (Heroic, Lutris). Their settings reach the game only when the launch starts the launcher itself. A launcher already running takes the game from its shortcut, and the variables go nowhere. That is the same silent gap as Steam's before S2. The panel should dim through-launch tiles for launcher games: `Panel::handed_over` extended to them. It is a small change, independent of the phases above.
- **Picture.**
  - Frame generation versus picture.md's In game frame generation: one at a time; the tile dims.
  - The FPS cap counts frames shown.
  - Resolution still reaches Steam games by the compositor's identification of the client, which doesn't depend on the hook.
- **Android.** Through-launch tiles are dimmed by the existing handover rule (M12). Keymaps reach Android games after A2.
- **Security** (`docs/security.md`).
  - No CEF debugging port is opened (Decky, CEF-driven Steam changes and Kettle's shortcut adder were all rejected).
  - The new root helper is fixed-function behind polkit, as `thor-account`.
  - Google's images come from Waydroid's servers on the user's request.

## 5. Designs that grow, and what was dropped

- **S3 (games installing in Steam).** Optional. It needs a watcher and a new entry state. Do it only if it is wanted after S1 and S2.
- **The tile menu's second line.** An extension of the shared menu widget. Keep it to one line per value.
- **Turnip.** Two drivers only. User-supplied driver folders were dropped: they need ELF checks, and they fail at the game's start.
- **FEX.** Presets only. The Env tile is for the rest.
- **Dropped:**
  - Game Mode, in either form.
  - Decky and CEF-based Steam control.
  - A Proton tile.
  - A game database or ratings.
  - Auto TDP.
  - UE5 ini edits.
  - Lepton.
  - Play by identity spoofing.
  - Aurora or microG shipped by us.
  - Steam Input UI in ClamShell.
  - A gyro-capable virtual controller for Steam.
- **Possible later and small:** reading Steam's per-game Steam Input state from `localconfig.vdf` and dimming the Keymap tile with "Steam Input maps this game", so layouts don't stack. Only if users stack them by mistake.

## 6. Documents to update with the work

- `docs/runners.md`: Steam's hook, the precedence order, Steam Input, `setup`.
- `docs/android.md`: setup through the Device view, Google Play.
- `docs/picture.md`: frame generation as a launch-time layer, OptiScaler for ARM64EC.
- `docs/state-machines.md`: the Device view's Steam and Android machines, and the settings additions with their tests.
- `docs/thor-gameplan.md`: the device checks above, under "Testing our system on the device".
- `docs/security.md`: the new helper.

### Critical Files for Implementation
- /home/robin/Code/thor/clamshell/crates/state/src/settings/mod.rs
- /home/robin/Code/thor/clamshell/crates/system/src/launch/steam.rs
- /home/robin/Code/thor/packages/thor-session/root/usr/bin/steam-setup
- /home/robin/Code/thor/clamshell/crates/state/src/device_view/mod.rs
- /home/robin/Code/thor/clamshell/crates/state/src/session/running.rs
