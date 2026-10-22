#!/usr/bin/env python3
"""RetroArch's menus in ClamShell's look: makes the assets folder retroarch.cfg points at
(~/.config/retroarch/clamshell-assets, or the folder given). The system's assets are read from
/usr/share/retroarch/assets, or the folder given second (an image's root's, while building one); the
links point at /usr/share/retroarch/assets either way.

- The system's assets (/usr/share/retroarch/assets) linked in, so everything else finds its files.
- XMB's monochrome icons in ClamShell's amber, settings' icons in its blue, those that end or remove
  something in red; their shapes (alpha) kept. XMB shows them as its custom theme (xmb_theme 6, a link to
  them), Ozone as its icons (on Linux it takes XMB's monochrome set).
- GLUI's (MaterialUI's) icons the same way, its font Noto Sans, as ClamShell's. Not the
  menu in use (XMB's is truly black; their darkest calm themes are grey), kept to switch to, in
  retroarch.cfg: GLUI with menu_driver "glui", materialui_menu_color_theme "22" (Gray Dark),
  materialui_show_nav_bar "false"; Ozone with menu_driver "ozone", ozone_menu_color_theme "10" (Gray
  Dark), ozone_header_separator "0", ozone_font "/usr/share/fonts/noto/NotoSans-Regular.ttf".
- black.png, XMB's wallpaper: the background black, not its gradient.

Run again after a RetroArch update brings new icons. Needs Pillow (python-pillow).

    menu-theme.py [folder [system assets]]
"""

import os
import shutil
import sys

from PIL import Image

SYSTEM = "/usr/share/retroarch/assets"
FONT = "/usr/share/fonts/noto/NotoSans-Regular.ttf"
AMBER, BLUE, RED = (0xE0, 0xA0, 0x40), (0x5F, 0x82, 0xB8), (0xD0, 0x60, 0x5A)
# Settings' icons and those that end or remove something, by their names in XMB's and GLUI's sets.
SETTINGS = {
    "setting", "settings", "subsetting", "core-options", "core-input-remapping-options", "core-shader-options",
    "core-cheat-options", "core-disk-options", "core-infos", "menu_info", "menu_drivers", "menu_video",
    "menu_audio", "menu_ui", "menu_network", "wifi", "bluetooth", "menu_latency", "menu_saving", "menu_osd",
    "core_options", "core_cheat_options", "controls", "shaders", "information", "configurations", "video", "help",
}
ENDS = {"close", "menu_exit", "menu_shutdown", "menu_power", "menu_record", "quit", "remove", "eject"}
# The icon sets made ours (the rest linked).
THEMED = {"xmb", "glui"}


def link(source, target):
    if not os.path.lexists(target):
        os.symlink(source, target)


def tint(src, dst):
    """The icons in `src` (all the way down) into `dst`, in our colours."""
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


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser("~/.config/retroarch/clamshell-assets")
    system = sys.argv[2] if len(sys.argv) > 2 else SYSTEM
    os.makedirs(out, exist_ok=True)
    for name in os.listdir(system):
        if name in THEMED:
            # (A link left by an older run: a folder of our own now.)
            if os.path.islink(f"{out}/{name}"):
                os.remove(f"{out}/{name}")
            os.makedirs(f"{out}/{name}", exist_ok=True)
        else:
            link(f"{SYSTEM}/{name}", f"{out}/{name}")
    # XMB: its monochrome set ours (its font and readme linked), the custom theme that set, the other themes
    # linked. (Older runs: monochrome a link, custom a copy.)
    for name in ("monochrome", "custom"):
        if os.path.islink(f"{out}/xmb/{name}") == (name == "monochrome"):
            shutil.rmtree(f"{out}/xmb/{name}", ignore_errors=True) if name == "custom" else os.remove(f"{out}/xmb/{name}")
    for name in os.listdir(f"{system}/xmb"):
        if name != "monochrome":
            link(f"{SYSTEM}/xmb/{name}", f"{out}/xmb/{name}")
    os.makedirs(f"{out}/xmb/monochrome", exist_ok=True)
    for name in os.listdir(f"{system}/xmb/monochrome"):
        if name != "png":
            link(f"{SYSTEM}/xmb/monochrome/{name}", f"{out}/xmb/monochrome/{name}")
    tint(f"{system}/xmb/monochrome/png", f"{out}/xmb/monochrome/png")
    link("monochrome", f"{out}/xmb/custom")
    # GLUI: its icons ours, its font Noto Sans, the rest linked.
    if os.path.isdir(f"{system}/glui"):
        for name in os.listdir(f"{system}/glui"):
            if not name.endswith(".png") and name != "font.ttf":
                link(f"{SYSTEM}/glui/{name}", f"{out}/glui/{name}")
        link(FONT, f"{out}/glui/font.ttf")
        tint(f"{system}/glui", f"{out}/glui")
    Image.new("RGB", (16, 16), (0, 0, 0)).save(f"{out}/black.png")


if __name__ == "__main__":
    main()
