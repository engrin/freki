# /// script
# requires-python = ">=3.10"
# dependencies = ["dmgbuild==1.6.7", "ds-store==1.3.3", "mac-alias==2.2.3"]
# ///
"""Makes freki's disk image with a styled Finder window (`cargo xtask dist` runs this).

usage: uv run --script xtask/dmg.py APP ICNS BACKGROUND OUTPUT

The window shows freki.app on the left and an arrow to the Applications folder on the
right, over BACKGROUND (a PNG; a BACKGROUND@2x.png next to it is used on Retina
screens). dmgbuild writes the window layout (.DS_Store) itself, without Finder.
uv installs the exact versions pinned above (dmgbuild has no further dependencies).
"""

import sys

from dmgbuild import build_dmg


def main() -> None:
    if len(sys.argv) != 5:
        sys.exit(__doc__.split("\n\n")[1])
    app, icns, background, output = sys.argv[1:]
    build_dmg(
        output,
        "freki",
        settings={
            "format": "UDZO",
            "files": [app],
            "symlinks": {"Applications": "/Applications"},
            "icon": icns,
            "background": background,
            # The background is 480 points high, the window less: the title bar may or
            # may not count towards the height, and the picture fills it either way.
            "window_rect": ((200, 120), (640, 428)),
            "icon_size": 128,
            "text_size": 13,
            "icon_locations": {"freki.app": (160, 180), "Applications": (480, 180)},
            "hide_extensions": ["freki.app"],
        },
    )


if __name__ == "__main__":
    main()
