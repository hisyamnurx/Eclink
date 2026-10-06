"""Generate Eclyzer Bridge application icons.

The mark is a neutral "bridge span" glyph in the Eclyzer palette. It
deliberately contains no OpenAI, Anthropic, or other third-party trademark,
per the PRD branding rules (PRD S58.2).

Run:  python scripts/generate_icons.py
"""

from __future__ import annotations

import os
from pathlib import Path

from PIL import Image, ImageDraw

# --- Eclyzer design tokens (PRD S36.1) -------------------------------------
BG_BASE = (10, 10, 11, 255)
BG_SURFACE = (19, 20, 23, 255)
ACCENT = (79, 140, 255, 255)
ACCENT_SOFT = (138, 180, 255, 255)

# Render at a high resolution, then downsample for crisp small sizes.
SUPERSAMPLE = 8
CANVAS = 1024
S = CANVAS * SUPERSAMPLE

OUT_DIR = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"


def rounded_rect(draw: ImageDraw.ImageDraw, box, radius, fill) -> None:
    draw.rounded_rectangle(box, radius=radius, fill=fill)


def draw_mark(size: int) -> Image.Image:
    """Render the bridge mark at `size` x `size` pixels (supersampled)."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    # Rounded square tile with a subtle vertical gradient baked in by drawing
    # two stacked rects and blending region-wise.
    margin = int(size * 0.055)
    radius = int(size * 0.22)
    rounded_rect(draw, (margin, margin, size - margin, size - margin), radius, BG_SURFACE)

    # A soft accent glow behind the glyph.
    glow = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    gdraw = ImageDraw.Draw(glow)
    gdraw.ellipse(
        (
            int(size * 0.18),
            int(size * 0.30),
            int(size * 0.82),
            int(size * 0.86),
        ),
        fill=(79, 140, 255, 46),
    )
    glow = glow.filter(__import__("PIL.ImageFilter", fromlist=["GaussianBlur"]).GaussianBlur(size * 0.05))
    img = Image.alpha_composite(img, glow)
    draw = ImageDraw.Draw(img)

    # --- The bridge glyph -------------------------------------------------
    # Two towers, a deck, and a suspension arc: abstract, legible at 32px.
    stroke = int(size * 0.052)
    left_x = int(size * 0.29)
    right_x = int(size * 0.71)
    deck_y = int(size * 0.645)
    tower_top = int(size * 0.315)
    tower_bottom = int(size * 0.735)

    # Deck (horizontal span).
    draw.rounded_rectangle(
        (left_x - stroke // 2, deck_y - stroke // 2, right_x + stroke // 2, deck_y + stroke // 2),
        radius=stroke // 2,
        fill=ACCENT,
    )

    # Towers.
    for x in (left_x, right_x):
        draw.rounded_rectangle(
            (x - stroke // 2, tower_top, x + stroke // 2, tower_bottom),
            radius=stroke // 2,
            fill=ACCENT_SOFT,
        )

    # Suspension arc between the towers, drawn as a true curve.
    arc_box = (
        left_x,
        int(size * 0.315),
        right_x,
        int(size * 0.685),
    )
    draw.arc(arc_box, start=200, end=340, fill=ACCENT, width=stroke)

    # Two hangers from the arc down to the deck.
    for t in (0.30, 0.70):
        x = int(left_x + (right_x - left_x) * t)
        arc_y = int(size * 0.335) + int((1 - (2 * t - 1) ** 2) * size * 0.075)
        draw.rounded_rectangle(
            (x - stroke // 3, arc_y, x + stroke // 3, deck_y),
            radius=stroke // 3,
            fill=ACCENT_SOFT,
        )

    return img


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    master = draw_mark(S)

    # Tauri expects this exact set for Windows bundles.
    png_sizes = {
        "32x32.png": 32,
        "128x128.png": 128,
        "128x128@2x.png": 256,
        "icon.png": 512,
        "Square30x30Logo.png": 30,
        "Square44x44Logo.png": 44,
        "Square71x71Logo.png": 71,
        "Square89x89Logo.png": 89,
        "Square107x107Logo.png": 107,
        "Square142x142Logo.png": 142,
        "Square150x150Logo.png": 150,
        "Square284x284Logo.png": 284,
        "Square310x310Logo.png": 310,
        "StoreLogo.png": 50,
    }

    for name, px in png_sizes.items():
        resized = master.resize((px, px), Image.LANCZOS)
        resized.save(OUT_DIR / name, format="PNG", optimize=True)

    # Multi-resolution .ico for the installer, taskbar, and window icon.
    ico_sizes = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
    master.resize((256, 256), Image.LANCZOS).save(
        OUT_DIR / "icon.ico", format="ICO", sizes=ico_sizes
    )

    print(f"wrote {len(png_sizes) + 1} icon files to {OUT_DIR}")


if __name__ == "__main__":
    main()
