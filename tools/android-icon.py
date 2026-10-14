"""Draws QuireInk's Android launcher icon: a white Literata "Q" on a black
rounded square, at every launcher density.

Usage: python tools/android-icon.py   (needs Pillow; writes
shells/android/res/mipmap-*/ic_launcher.png)
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
FONT = ROOT / "plugins/fonts-default/fonts/Literata.ttf"
RES = ROOT / "shells/android/res"
SIZES = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}
# Drawn this much larger, then reduced: smooth edges at every size.
SCALE = 8


def icon(size: int) -> Image.Image:
    big = size * SCALE
    image = Image.new("L", (big, big), 0)
    mask = Image.new("L", (big, big), 0)
    # The square fills 88 % of the icon, as launchers expect.
    inset = round(big * 0.06)
    box = (inset, inset, big - inset, big - inset)
    ImageDraw.Draw(mask).rounded_rectangle(box, radius=round(big * 0.2), fill=255)
    font = ImageFont.truetype(str(FONT), round(big * 0.66))
    try:
        font.set_variation_by_name("Bold")
    except (OSError, ValueError):
        pass
    draw = ImageDraw.Draw(image)
    left, top, right, bottom = draw.textbbox((0, 0), "Q", font=font)
    x = (big - (right - left)) / 2 - left
    # Optically centred: the tail of the Q hangs below.
    y = (big - (bottom - top)) / 2 - top - big * 0.02
    draw.text((x, y), "Q", font=font, fill=255)
    black = Image.new("L", (big, big), 20)
    white = Image.new("L", (big, big), 250)
    face = Image.composite(white, black, image)
    rgba = Image.merge("RGBA", (face, face, face, mask))
    return rgba.resize((size, size), Image.LANCZOS)


def main() -> None:
    for density, size in SIZES.items():
        out = RES / f"mipmap-{density}" / "ic_launcher.png"
        out.parent.mkdir(parents=True, exist_ok=True)
        icon(size).save(out, optimize=True)
        print(out.relative_to(ROOT))


if __name__ == "__main__":
    main()
