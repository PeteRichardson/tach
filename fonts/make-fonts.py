# /// script
# requires-python = ">=3.14"
# dependencies = ["pillow>=11"]
# ///
"""Render the device fonts to BDF from Monaspace Neon (docs/device-v2.md, "Transport").

The firmware bakes these BDF files into its glyph atlas at build time
(device/glyphs.rs), so this script runs only when the font, the size or the
character set changes. The BDF files are committed; the OTF file is not.

    uv run fonts/make-fonts.py "<dir>/MonaspaceNeon-SemiBold.otf"

Get the OTF from the static release at https://github.com/githubnext/monaspace.
The OFL reserves the names "Monaspace" and "Neon", so the output is named
"Tach" (OFL.txt, condition 3).
"""
import sys
from collections import Counter
from pathlib import Path

from PIL import ImageFont

HERE = Path(__file__).parent

# (file, pixel size, characters). Digits: Q34, 36 px. Labels: the three row names.
FONTS = [
    ("tach-36.bdf", 36, "0123456789.%G-"),
    ("tach-16.bdf", 16, "CEGMPU"),
]


def glyph(font, ch):
    """(advance, x offset, y offset, rows) of one character, trimmed to its ink.

    Offsets are BDF style: x from the pen position, y from the baseline up to
    the bottom row. Rows are strings of '0' and '1'.
    """
    mask, (ox, oy) = font.getmask2(ch, mode="1", anchor="ls")
    w, h = mask.size
    rows = ["".join("1" if mask.getpixel((x, y)) else "0" for x in range(w)) for y in range(h)]
    # Trim empty rows and columns, so the atlas holds ink only.
    while rows and "1" not in rows[0]:
        rows.pop(0)
        oy += 1
    while rows and "1" not in rows[-1]:
        rows.pop()
    if not rows:
        return round(font.getlength(ch)), 0, 0, []
    left = min(r.index("1") for r in rows)
    right = max(r.rindex("1") for r in rows) + 1
    rows = [r[left:right] for r in rows]
    return round(font.getlength(ch)), ox + left, -(oy + len(rows)), rows


def bdf(font, name, size, chars, source):
    ascent, descent = font.getmetrics()
    glyphs = [(ch, *glyph(font, ch)) for ch in chars]
    # Hinting puts some digits 1 px off the others: in SemiBold at 36 px, the "0" sat 1 px high,
    # and a number jumped when a 0 changed to another digit. Digits change in place, so they all
    # take the bottom row that most of them have.
    bottom = Counter(g[3] for g in glyphs if g[0].isdigit()).most_common(1)
    if bottom:
        glyphs = [(ch, adv, xo, bottom[0][0] if ch.isdigit() else yo, rows)
                  for ch, adv, xo, yo, rows in glyphs]
    out = [
        "STARTFONT 2.1",
        f"FONT -Tach-{name}-{size}",
        f"SIZE {size} 75 75",
        f"FONTBOUNDINGBOX {max(len(g[4][0]) if g[4] else 0 for g in glyphs)} {ascent + descent} 0 {-descent}",
        "COMMENT Rendered by fonts/make-fonts.py from " + source,
        "COMMENT Derived from Monaspace, Copyright (c) 2023, GitHub. SIL Open Font License 1.1: fonts/OFL.txt",
        "STARTPROPERTIES 4",
        f"PIXEL_SIZE {size}",
        f"FONT_ASCENT {ascent}",
        f"FONT_DESCENT {descent}",
        'COPYRIGHT "Copyright (c) 2023, GitHub. Derived from Monaspace. SIL Open Font License 1.1"',
        "ENDPROPERTIES",
        f"CHARS {len(glyphs)}",
    ]
    for ch, adv, xo, yo, rows in glyphs:
        w = len(rows[0]) if rows else 0
        out += [
            f"STARTCHAR U+{ord(ch):04X}",
            f"ENCODING {ord(ch)}",
            f"SWIDTH {adv * 1000 // size} 0",
            f"DWIDTH {adv} 0",
            f"BBX {w} {len(rows)} {xo} {yo}",
            "BITMAP",
        ]
        nbytes = (w + 7) // 8
        out += [f"{int(r.ljust(nbytes * 8, '0'), 2):0{nbytes * 2}X}" for r in rows]
        out.append("ENDCHAR")
    out.append("ENDFONT")
    return "\n".join(out) + "\n"


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    otf = Path(sys.argv[1])
    for file, size, chars in FONTS:
        font = ImageFont.truetype(str(otf), size)
        (HERE / file).write_text(bdf(font, Path(file).stem, size, chars, otf.name))
        print(f"wrote fonts/{file}")


if __name__ == "__main__":
    main()
