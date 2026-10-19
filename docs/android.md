# Android apps (Waydroid)

Waydroid runs Android (LineageOS 20, Android 13) in a container on the host's kernel, its apps drawn as
Wayland windows: one window per app in its multi-window mode. Each app has a desktop file
(`~/.local/share/applications/waydroid.<package>.desktop`), so ClamShell lists them as desktop apps, and
their windows' app id is the desktop file's name (`waydroid.<package>`). The windows are the Waydroid
session's, not the launch's: the runner's `handover` has ClamShell know them by that app id, as it does
Steam's games (see runners.md: Launches). An app opened by another Android app is on the dock too.

The starter library.json opens them with the Android runner, `clamshell/android/clamshell-android`:

- Starts the Waydroid session if it isn't running (a user unit, `waydroid-session`, bound to the Wayland
  filter's unit, see below; it stays up when apps quit). The first launch after a session start takes
  12–20 s, Android's boot; later ones about 1 s. On a cold start Waydroid opens the app's window, closes it
  and opens another; ClamShell shows a window that comes on the workspace whose window in front closed
  within 2 s.
- Starts the app full-screen through adb (a user unit, `clamshell-adb`), as Android's shell user. Waydroid's
  own launch opens apps as Android's floating windows, phone-sized in the middle of the window.
- Waydroid shows no app windows until `waydroid.active_apps` names an app, and only Waydroid's own launch
  sets it: a session's first launch runs it once with the launcher, which Waydroid never shows.

The dock menu has no Open another for them (a launch shows the app that runs). Its cost line, and the
battery tab's row, are the app's own processes in Waydroid's container (Android names them after the
package): their CPU and memory; their GPU time is another user's, unreadable, so a dash. The rest of
the container's processes are one row, Android. In the settings, the Android runner's tab and its apps'
dim the FPS cap, the HUD and the variables ("no effect"): they work through the launch's own process,
which hands the app over and ends.

Quit closes the window, then runs the runner's `stop` once it is gone (the entry ends with its window);
Force kill runs the `stop` at once. The `stop` is the script's `--stop`: Android's `am force-stop`, and
the app's tasks removed (`am stack remove`): a task can outlive the app with another app's activity on
top, Android's permission prompt, and its window with it.

```json
"runners": [
  { "type": "desktop", "id": "android", "name": "Android", "launch": ["~/.local/bin/clamshell-android", "{id}"], "handover": true,
    "stop": ["~/.local/bin/clamshell-android", "--stop", "{id}"] }
],
"sources": [
  { "type": "desktop", "id": "Waydroid", "hide": true },
  { "type": "desktop", "id": "waydroid.*", "place": "@apps", "runner": "android" }
]
```

(The `Waydroid` entry is the whole Android screen in one window; the sources go before the catch-all desktop
source.)

## Typing

The clipboard is shared both ways: Waydroid's session imports `pyclip` for it, and the script starts the
session with a stand-in of ClamShell's on its Python path (`clamshell/android/pyclip.py`, installed in
`~/.local/share/clamshell/python`), which reads and sets the clipboard through ClamShell (`clamshell
--clipboard get|set`), so no other package is needed. Tried: text copied on the host pasted into
F-Droid's search field, and text copied there read on the host.

Android sees a hardware keyboard (Waydroid's Wayland keyboard), so ClamShell's keyboard types into
Android apps and Android's own on-screen keyboard is turned off (`show_ime_with_hard_keyboard 0`, below).
Waydroid has no text-input protocol, so a text field getting focus doesn't raise ClamShell's keyboard:
open it from the bar.

## Windows kept: the Wayland filter

Waydroid answers every `xdg_toplevel.configure` with a display hotplug (`xdg_toplevel_handle_configure`
calls `do_hotplug` whatever the size), which clears its windows; it draws a window again only with its
app's next frame, and an app at rest draws none. Sway configures a window as it is hidden, shown,
focused or unfocused, its size unchanged: an Android app's window was lost whenever it went out of
sight or lost focus, and didn't come back (Android keeping the task).

So Waydroid connects through ClamShell's Wayland filter (`clamshell --wayland-filter`, its own unit
`clamshell-wayland-filter-<display>`, started by the script before the session): a proxy on a socket
beside the compositor's (`<display>-android`) that passes everything through but a toplevel configure of
the size the window already has. A resize still reaches Waydroid. (The fix belongs in Waydroid: a
hotplug only when the size changed.)

If the compositor ends a connection (it quit, or dropped Waydroid), the filter ends, and the Waydroid
session with it (bound to the filter's unit): Waydroid can't go on without its display. Tried: with sway
restarted under a running Android, Android's system crashed when it next drew (`system_server` restarted)
and its network didn't come back.

It does two more things for Android's windows:

- Android's caption (back, minimise, maximise, close, drawn into the window along its top) is out of
  sight: Android is told a window the caption's height taller, and the compositor a window that starts
  below it (its window geometry), so the app's own area fills the window, unscaled; the app lays itself
  out below the caption, so nothing of it is under it. Its height is in `android-caption` in ClamShell's
  runtime dir: the script writes it once Android has started, 57 pixels at a density of 213 (measured) in
  proportion. Back: the pad's B (Escape). A new geometry is the compositor's from the app's next frame.
- The app's frame rate (its window's commits, the window in front) is written as MangoHud logs it
  (`android-<app id>.csv` in ClamShell's MangoHud folder, a row every half second), which ClamShell
  reads as it does a game's.

No frame rate cap: Waydroid paces Android by a vsync of its own, its period fixed when the session starts
(from the outputs' refresh), not by the compositor's frame callbacks. A cap would be all of Android's, set
as the session starts.

Its cost, measured on the desktop with an app drawing at 50 fps: 0.1 % of a core, 6 MB.

## In the background

Android's apps share one container, so they are paused together: the Android runner's `pause` and
`resume` (the script's `--pause`, `--resume`: Waydroid's container service freezes and thaws the
container's cgroup, no root needed). With In the background set to Pauses hidden for the Android runner,
Android is frozen once none of its apps has been shown for 10 s (none running too) and none is starting
(launched, no window yet: it would never draw one), and thawed as one is shown or launched; a launch thaws
it first in any case. Android's own sleep (it asks to suspend once no window of it is shown, and Waydroid
freezes the container: `persist.waydroid.suspend`) is turned off at setup, so that ClamShell's setting
decides.

## Not solved

- Android draws its own caption on each window (back, minimise, maximise, close), and Waydroid has no
  option to turn it off (waydroid/waydroid#2204): the Wayland filter keeps it out of sight (above).
- GPU time per app: Android's processes are other users', their DRM counters unreadable without root;
  Android's own GPU accounting needs a kernel event Mesa's drivers don't emit.
- Rendering needs Mesa's drivers (Intel, AMD; the Thor's Freedreno untested). The NVIDIA driver and VMs
  without virgl need software rendering (below).

## Setup

As root once; then per user. The kernel needs binder (`zgrep BINDER_IPC /proc/config.gz`; Arch's and
CachyOS's have it).

```sh
sudo pacman -S waydroid android-tools
sudo waydroid init                      # Android's image, without Google's apps
sudo systemctl enable --now waydroid-container
```

Waydroid's network is a bridge (`waydroid0`) with its own DHCP and DNS (dnsmasq) on the host: a kernel
updated since the last boot has no bridge module (reboot), and a firewall must let them through, or
Android gets no address (`waydroid status`: UNKNOWN) and adb can't reach it. On the desktop both ufw and
the nftables service's own table (`/etc/nftables.conf`) drop what isn't allowed, so both needed it:

```sh
# ufw
sudo ufw allow in on waydroid0 to any port 67 proto udp
sudo ufw allow in on waydroid0 to any port 53
sudo ufw route allow in on waydroid0
sudo ufw route allow out on waydroid0
```

```
# /etc/nftables.conf, in table inet filter (then `sudo nft -f /etc/nftables.conf`)
chain input:   iifname "waydroid0" udp dport { 53, 67 } accept
               iifname "waydroid0" tcp dport 53 accept
chain forward: iifname "waydroid0" accept
               oifname "waydroid0" ct state { established, related } accept
```

The rest runs where ClamShell's display is (on the desktop, in a terminal started from the nested
session's shelf), with the session running (`waydroid session start &`, until "Android with user 0 is
ready"). Android without a window shown suspends at once (Waydroid freezes the container): thaw it
first, and turn that off, so that what pauses Android is ClamShell's In the background:

```sh
busctl call id.waydro.Container /ContainerManager id.waydro.ContainerManager Unfreeze
waydroid prop set persist.waydroid.suspend false
# One window per app.
waydroid prop set persist.waydroid.multi_windows true
# adb's key allowed in Android (adb makes it at its first start). (Piped in: lxc-attach takes its
# standard input and output for root, a file redirected there too.)
adb start-server
cat ~/.android/adbkey.pub | sudo lxc-attach -P /var/lib/waydroid/lxc -n waydroid --clear-env -- /system/bin/sh -c \
    'cat >> /data/misc/adb/adb_keys; chmod 640 /data/misc/adb/adb_keys; chown system:shell /data/misc/adb/adb_keys' | cat
adb connect "$(waydroid status | sed -n 's/^IP address:[[:space:]]*//p'):5555"
# Android's on-screen keyboard off with a hardware keyboard; no notification pop-ups over apps.
adb shell settings put secure show_ime_with_hard_keyboard 0
adb shell settings put global heads_up_notifications_enabled 0
waydroid session stop
```

Then the runner: the session script installs it (`~/.local/bin/clamshell-android`); on a desktop,
`install -m 755 clamshell/android/clamshell-android ~/.local/bin/`, and the runner and sources above in
`~/.config/clamshell/library.json`. Apps installed later (`waydroid app install app.apk`) are listed after a
rescan (Tools).

Software rendering (NVIDIA, a VM without virgl): in `/var/lib/waydroid/waydroid_base.prop`, as root,
`ro.hardware.gralloc=default` and `ro.hardware.egl=swiftshader`, then restart the container
(`sudo systemctl restart waydroid-container`).
