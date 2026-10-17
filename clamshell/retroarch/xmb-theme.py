#!/usr/bin/env python3
"""RetroArch's menu in ClamShell's look: makes the assets folder retroarch.cfg points at
(~/.config/retroarch/clamshell-assets, or the folder given). The system's assets are read from
/usr/share/retroarch/assets, or the folder given second (an image's root's, while building one); the
links point at /usr/share/retroarch/assets either way.

- The system's assets (/usr/share/retroarch/assets) linked in, so everything else finds its files.
- XMB's custom icon theme (xmb_theme 6): its monochrome icons in ClamShell's amber, settings' icons in its
  blue, those that end or remove something in red; their shapes (alpha) kept.
- black.png, the menu's wallpaper: the background black, not XMB's gradient.

Run again after a RetroArch update brings new icons. Needs Pillow (python-pillow).

    clamshell/retroarch/xmb-theme.py [folder [system assets]]
"""

import os
import sys

from PIL import Image

SYSTEM = "/usr/share/retroarch/assets"
AMBER, BLUE, RED = (0xE0, 0xA0, 0x40), (0x5F, 0x82, 0xB8), (0xD0, 0x60, 0x5A)
SETTINGS = {
    "setting", "settings", "subsetting", "core-options", "core-input-remapping-options", "core-shader-options",
    "core-cheat-options", "core-disk-options", "core-infos", "menu_info", "menu_drivers", "menu_video",
    "menu_audio", "menu_ui", "menu_network", "wifi", "bluetooth", "menu_latency", "menu_saving", "menu_osd",
}
ENDS = {"close", "menu_exit", "menu_shutdown", "menu_power", "menu_record"}


def link(source, target):
    if not os.path.lexists(target):
        os.symlink(source, target)


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser("~/.config/retroarch/clamshell-assets")
    system = sys.argv[2] if len(sys.argv) > 2 else SYSTEM
    os.makedirs(f"{out}/xmb", exist_ok=True)
    for name in os.listdir(system):
        if name != "xmb":
            link(f"{SYSTEM}/{name}", f"{out}/{name}")
    for name in os.listdir(f"{system}/xmb"):
        link(f"{SYSTEM}/xmb/{name}", f"{out}/xmb/{name}")
    src, dst = f"{system}/xmb/monochrome/png", f"{out}/xmb/custom/png"
    for root, _, files in os.walk(src):
        os.makedirs(root.replace(src, dst, 1), exist_ok=True)
        for file in files:
            if not file.endswith(".png"):
                continue
            name = file[:-4]
            colour = BLUE if name in SETTINGS else RED if name in ENDS else AMBER
            icon = Image.open(os.path.join(root, file)).convert("RGBA")
            tinted = Image.new("RGBA", icon.size, colour + (255,))
            tinted.putalpha(icon.getchannel("A"))
            tinted.save(os.path.join(root.replace(src, dst, 1), file))
    Image.new("RGB", (16, 16), (0, 0, 0)).save(f"{out}/black.png")


if __name__ == "__main__":
    main()
