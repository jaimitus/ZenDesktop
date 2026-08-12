#!/usr/bin/env python3
"""ZenDesktop :: generador de iconos.

Dibuja la marca "Fences" de ZenDesktop: una loseta redondeada oscura con una
cuadricula 2x2 de cuadrados redondeados en los cuatro colores de acento de la
app (azul cielo, violeta, rosa y verde), mas una variante de bandeja sobre
fondo transparente.

Renderiza con 4x de supersampling y produce:
  * assets/icons/zendesktop.ico         (multi-tamano, para el .exe)
  * assets/icons/zendesktop-tray.ico    (variante bandeja del sistema)
  * assets/icons/png/*.png              (16 .. 1024, variante app)
  * assets/icons/tray/*.png             (16 .. 256, variante bandeja)
  * assets/icons/preview.html           (vista previa para revision)

Uso:  python tools/gen_icons.py
Requiere: Pillow (pip install Pillow)
"""

import os
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFilter

# ---------------------------------------------------------------------------
# Configuracion
# ---------------------------------------------------------------------------

SS = 4          # factor de supersampling (calidad de bordes)
BASE = 1024     # tamano del master (px)

# Colores de acento por caja: (claro arriba, oscuro abajo). Coinciden con los
# colores por defecto de config.toml (media, docs, setup, misc).
ACCENTS = [
    ("#7DD3FC", "#0284C7"),   # Media        (sky)
    ("#C4B5FD", "#7C3AED"),   # Documentos   (violet)
    ("#F9A8D4", "#DB2777"),   # Instaladores (pink)
    ("#6EE7B7", "#059669"),   # Varios       (emerald)
]

TILE_TOP = "#2B3550"   # fondo de la loseta, arriba
TILE_BOT = "#0A0F1F"   # fondo de la loseta, abajo

APP_SIZES = [16, 24, 32, 48, 64, 128, 256, 512, 1024]
ICO_SIZES = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]
TRAY_SIZES = [16, 24, 32, 48, 64, 128, 256]

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICON_DIR = os.path.join(ROOT, "assets", "icons")
PNG_DIR = os.path.join(ICON_DIR, "png")
TRAY_DIR = os.path.join(ICON_DIR, "tray")


# ---------------------------------------------------------------------------
# Primitivas de dibujo
# ---------------------------------------------------------------------------

def hex2rgb(h):
    h = h.lstrip("#")
    return tuple(int(h[i : i + 2], 16) for i in (0, 2, 4))


def lerp(c1, c2, t):
    return tuple(int(a + (b - a) * t) for a, b in zip(c1, c2))


def vgrad(w, h, top, bottom):
    """Gradiente vertical (top -> bottom) del tamano indicado."""
    top, bottom = hex2rgb(top), hex2rgb(bottom)
    strip = Image.new("RGB", (1, h))
    d = ImageDraw.Draw(strip)
    for y in range(h):
        d.line([(0, y), (0, y)], fill=lerp(top, bottom, y / max(h - 1, 1)))
    return strip.resize((w, h), Image.BILINEAR)


def rounded_mask(size, radius):
    mask = Image.new("L", size, 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [0, 0, size[0] - 1, size[1] - 1], radius=radius, fill=255
    )
    return mask


def gradient_shape(size, radius, top, bottom):
    """Rectangulo redondeado con gradiente vertical y alfa total."""
    img = vgrad(size[0], size[1], top, bottom).convert("RGBA")
    img.putalpha(rounded_mask(size, radius))
    return img


def top_highlight(size, radius, depth=0.30, strength=38):
    """Brillo superior: blanco que se desvanece hacia abajo, recortado."""
    w, h = size
    hl = Image.new("RGBA", size, (255, 255, 255, 0))
    ImageDraw.Draw(hl).rounded_rectangle(
        [0, 0, w - 1, h - 1], radius=radius, fill=(255, 255, 255, strength)
    )
    fade = Image.new("L", size, 0)
    d = ImageDraw.Draw(fade)
    band = int(h * depth)
    for y in range(band):
        d.line([(0, y), (w, y)], fill=int(255 * (1 - y / band)))
    # Escala el desvanecimiento a la intensidad deseada (0..strength).
    fade = ImageChops.multiply(rounded_mask(size, radius), fade).point(
        lambda v: v * strength // 255
    )
    hl.putalpha(fade)
    return hl


def shadow_rect(size, radius, offset, alpha=90, blur=0.0):
    """Sombra suave de un rectangulo redondeado (capa RGBA transparente)."""
    w, h = size
    sh = Image.new("RGBA", size, (0, 0, 0, 0))
    d = ImageDraw.Draw(sh)
    d.rounded_rectangle(
        [offset[0], offset[1], w - 1 + offset[0], h - 1 + offset[1]],
        radius=radius,
        fill=(0, 0, 0, alpha),
    )
    if blur > 0:
        sh = sh.filter(ImageFilter.GaussianBlur(blur))
    return sh


# ---------------------------------------------------------------------------
# Composicion de la marca
# ---------------------------------------------------------------------------

def draw_app_tile(size):
    """Loseta completa (icono de aplicacion / archivo)."""
    S = size
    canvas = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    m = int(0.045 * S)                     # margen de la loseta
    R = int(0.235 * S)                     # radio de esquina de la loseta
    tile_rect = [m, m, S - 1 - m, S - 1 - m]

    # 1) Sombra proyectada de la loseta.
    canvas.alpha_composite(
        shadow_rect((S, S), R, (int(0.02 * S), int(0.035 * S)), alpha=80, blur=3 * SS)
    )

    # 2) Fondo con gradiente.
    tile = gradient_shape((S, S), R, TILE_TOP, TILE_BOT)
    canvas.alpha_composite(tile)

    # 3) Brillo superior del cristal.
    canvas.alpha_composite(top_highlight((S, S), R, depth=0.42, strength=26))

    # 4) Borde interior fino.
    bw = max(1, SS // 2)
    d = ImageDraw.Draw(canvas)
    d.rounded_rectangle(
        [tile_rect[0] + bw, tile_rect[1] + bw, tile_rect[2] - bw, tile_rect[3] - bw],
        radius=R - bw,
        outline=(255, 255, 255, 24),
        width=bw,
    )

    # 5) Cuadricula de cajas.
    T = 0.34 * S      # tamano de cada caja
    G = 0.05 * S      # hueco entre cajas
    grid = 2 * T + G
    o = (S - grid) / 2
    box_radius = T * 0.26
    positions = [
        (o, o),
        (o + T + G, o),
        (o, o + T + G),
        (o + T + G, o + T + G),
    ]

    # Sombra de las cajas (una sola capa, un solo blur).
    shadows = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    sd = ImageDraw.Draw(shadows)
    for px, py in positions:
        off = int(0.012 * S)
        sd.rounded_rectangle(
            [px, py + off, px + T - 1, py + T - 1 + off],
            radius=box_radius,
            fill=(0, 0, 0, 110),
        )
    shadows = shadows.filter(ImageFilter.GaussianBlur(2 * SS))
    canvas.alpha_composite(shadows)

    for (px, py), (top, bot) in zip(positions, ACCENTS):
        box = gradient_shape((S, S), box_radius, top, bot)
        canvas.alpha_composite(box, (int(px), int(py)))
        hl = top_highlight((S, S), box_radius, depth=0.32, strength=52)
        canvas.alpha_composite(hl, (int(px), int(py)))

    return canvas


def draw_tray(size):
    """Variante de bandeja: cuadricula flotante sobre fondo transparente."""
    S = size
    canvas = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    T = 0.30 * S
    G = 0.10 * S
    grid = 2 * T + G
    o = (S - grid) / 2
    box_radius = T * 0.30
    positions = [
        (o, o),
        (o + T + G, o),
        (o, o + T + G),
        (o + T + G, o + T + G),
    ]

    shadows = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    sd = ImageDraw.Draw(shadows)
    for px, py in positions:
        off = int(0.014 * S)
        sd.rounded_rectangle(
            [px, py + off, px + T - 1, py + T - 1 + off],
            radius=box_radius,
            fill=(0, 0, 0, 120),
        )
    shadows = shadows.filter(ImageFilter.GaussianBlur(2 * SS))
    canvas.alpha_composite(shadows)

    for (px, py), (top, bot) in zip(positions, ACCENTS):
        box = gradient_shape((S, S), box_radius, top, bot)
        canvas.alpha_composite(box, (int(px), int(py)))
        hl = top_highlight((S, S), box_radius, depth=0.34, strength=58)
        canvas.alpha_composite(hl, (int(px), int(py)))

    return canvas


# ---------------------------------------------------------------------------
# Salida
# ---------------------------------------------------------------------------

def ensure_dirs():
    for d in (ICON_DIR, PNG_DIR, TRAY_DIR):
        os.makedirs(d, exist_ok=True)


def write_png(img, path):
    img.save(path, format="PNG")
    print(f"  png  {os.path.relpath(path, ROOT)}")


def write_ico(img, path, sizes):
    img.save(path, format="ICO", sizes=sizes)
    print(f"  ico  {os.path.relpath(path, ROOT)}")


def build_preview():
    """HTML que muestra todas las variantes sobre fondo claro y oscuro."""
    rows = []
    for name, sizes in (
        ("App", APP_SIZES),
        ("Bandeja", TRAY_SIZES),
    ):
        sub = "png" if name == "App" else "tray"
        cells = []
        for s in sizes:
            cells.append(
                f'<td><img src="{sub}/{s}.png" width="{min(s, 96)}" '
                f'height="{min(s, 96)}" loading="lazy"><div class="cap">{s}px</div></td>'
            )
        rows.append(
            f'<tr><th>{name}</th>{"".join(cells)}</tr>'
        )
    html = f"""<!doctype html><html lang="es"><head><meta charset="utf-8">
<title>ZenDesktop - iconos</title>
<style>
  body {{ font-family: 'Segoe UI', sans-serif; margin: 24px; background: #10141f; color: #e6edf7; }}
  h1 {{ font-size: 18px; }} h2 {{ font-size: 14px; color: #8fa6c4; margin-top: 28px; }}
  table {{ border-collapse: collapse; }}
  td {{ text-align: center; padding: 14px 10px; }}
  th {{ text-align: left; padding: 6px 12px; color: #7c8aa0; font-weight: 600; }}
  .swatch {{ display: flex; align-items: center; justify-content: center; padding: 10px; }}
  .light {{ background: #f2f4f8; }} .dark {{ background: #141821; }}
  .cap {{ font-size: 11px; color: #7c8aa0; margin-top: 6px; }}
  .ico {{ width: 64px; height: 64px; }}
</style></head><body>
<h1>ZenDesktop &mdash; iconos</h1>
<h2>Variante aplicaci&oacute;n (loseta)</h2>
<div class="swatch light"><img src="png/256.png" width="96" height="96"></div>
<table>{rows[0]}</table>
<h2>Variante bandeja del sistema (transparente)</h2>
<div class="swatch dark"><img src="tray/256.png" width="96" height="96"></div>
<table>{rows[1]}</table>
</body></html>"""
    path = os.path.join(ICON_DIR, "preview.html")
    with open(path, "w", encoding="utf-8") as f:
        f.write(html)
    print(f"  html {os.path.relpath(path, ROOT)}")


def main():
    ensure_dirs()
    print("Renderizando master (supersampling x%d)..." % SS)
    master = draw_app_tile(BASE * SS).resize((BASE, BASE), Image.LANCZOS)
    tray = draw_tray(BASE * SS).resize((BASE, BASE), Image.LANCZOS)

    print("Variante app:")
    write_ico(master, os.path.join(ICON_DIR, "zendesktop.ico"), ICO_SIZES)
    for s in APP_SIZES:
        write_png(master.resize((s, s), Image.LANCZOS), os.path.join(PNG_DIR, f"{s}.png"))

    print("Variante bandeja:")
    write_ico(tray, os.path.join(ICON_DIR, "zendesktop-tray.ico"), ICO_SIZES)
    for s in TRAY_SIZES:
        write_png(tray.resize((s, s), Image.LANCZOS), os.path.join(TRAY_DIR, f"{s}.png"))

    build_preview()
    print("Listo.")


if __name__ == "__main__":
    sys.exit(main())
