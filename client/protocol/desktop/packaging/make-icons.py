"""Makes the desktop packages' icons from the app's store icon.

    python make-icons.py

fastlane/metadata/android/en-US/images/icon.png (512 px, the moon and the nightjar on a square)
gets rounded corners, then becomes icons/<size>.png for the Linux packages (hicolor sizes),
icons/nightjar.ico for the Windows installer, shortcuts and the uninstall entry, and
icons/wizard.png, the installer's side panel (the icon on its dark background). Needs Pillow.
The results are committed; run it again only when the store icon changes.
"""

from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parents[2] / "fastlane" / "metadata" / "android" / "en-US" / "images" / "icon.png"
OUT = HERE / "icons"
PNG_SIZES = (16, 24, 32, 48, 64, 128, 256, 512)
ICO_SIZES = (16, 20, 24, 32, 40, 48, 64, 128, 256)


def rounded(image: Image.Image) -> Image.Image:
    """The icon with corners of a fifth of its side, drawn four times larger for smooth edges."""
    size = image.width
    scale = 4
    mask = Image.new("L", (size * scale, size * scale), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (0, 0, size * scale - 1, size * scale - 1), radius=size * scale // 5, fill=255
    )
    out = image.convert("RGBA")
    out.putalpha(mask.resize((size, size), Image.Resampling.LANCZOS))
    return out


def main() -> None:
    icon = rounded(Image.open(SOURCE))
    OUT.mkdir(exist_ok=True)
    for size in PNG_SIZES:
        icon.resize((size, size), Image.Resampling.LANCZOS).save(OUT / f"{size}.png", optimize=True)
    icon.save(OUT / "nightjar.ico", sizes=[(s, s) for s in ICO_SIZES])
    # Inno Setup's side panel is 164 x 314 at 100 %; twice that stays sharp at 200 %.
    source = Image.open(SOURCE).convert("RGBA")
    panel = Image.new("RGBA", (328, 628), source.getpixel((0, 0)))
    mark = source.resize((256, 256), Image.Resampling.LANCZOS)
    panel.alpha_composite(mark, ((328 - 256) // 2, (628 - 256) // 2))
    panel.convert("RGB").save(OUT / "wizard.png", optimize=True)


if __name__ == "__main__":
    main()
