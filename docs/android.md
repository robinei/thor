# Android apps (Waydroid)

Waydroid runs Android (LineageOS 20, Android 13) in a container on the host's kernel, its apps drawn as
Wayland windows: one window per app in its multi-window mode. Each app has a desktop file
(`~/.local/share/applications/waydroid.<package>.desktop`), so ClamShell lists them as desktop apps, and
their windows' app id is the desktop file's name (`waydroid.<package>`), so a launch finds its window.

The starter library.json opens them with the Android runner, `clamshell/android/clamshell-android`:

- Starts the Waydroid session if it isn't running (a user unit, `waydroid-session`; it stays up when
  apps quit). The first launch after a session start takes about 20 s, Android's boot; later ones about 1 s.
- Starts the app full-screen through adb (a user unit, `clamshell-adb`), as Android's shell user. Waydroid's
  own launch opens apps as Android's floating windows, phone-sized in the middle of the window.
- Waydroid shows no app windows until `waydroid.active_apps` names an app, and only Waydroid's own launch
  sets it: a session's first launch runs it once with the launcher, which Waydroid never shows.

Quit closes the window, which ends the app's Android task (its process stays cached, as Android does).

```json
"runners": [
  { "type": "desktop", "id": "android", "name": "Android", "launch": ["~/.local/bin/clamshell-android", "{id}"] }
],
"sources": [
  { "type": "desktop", "id": "Waydroid", "hide": true },
  { "type": "desktop", "id": "waydroid.*", "place": "@apps", "runner": "android" }
]
```

(The `Waydroid` entry is the whole Android screen in one window; the sources go before the catch-all desktop
source.)

## Typing

Android sees a hardware keyboard (Waydroid's Wayland keyboard), so ClamShell's keyboard types into
Android apps and Android's own on-screen keyboard is turned off (`show_ime_with_hard_keyboard 0`, below).
Waydroid has no text-input protocol, so a text field getting focus doesn't raise ClamShell's keyboard:
open it from the bar.

## Not solved

- Android draws its own caption on each window (back, minimise, maximise, close). Waydroid has no option
  to turn it off (waydroid/waydroid#2204).
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

The rest runs where ClamShell's display is (on the desktop, in a terminal started from the nested
session's shelf), with the session running (`waydroid session start &`, until "Android with user 0 is
ready"):

```sh
# One window per app.
waydroid prop set persist.waydroid.multi_windows true
# adb's key allowed in Android (adb makes it at its first start).
adb start-server
sudo lxc-attach -P /var/lib/waydroid/lxc -n waydroid --clear-env -- /system/bin/sh -c \
    'cat >> /data/misc/adb/adb_keys; chmod 640 /data/misc/adb/adb_keys; chown system:shell /data/misc/adb/adb_keys' \
    < ~/.android/adbkey.pub
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
