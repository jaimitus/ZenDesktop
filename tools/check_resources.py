#!/usr/bin/env python3
"""ZenDesktop :: verificacion de recursos del .exe.

Inspecciona la tabla de recursos del ejecutable y lista los iconos
(RT_ICON / RT_GROUP_ICON) y el version info (RT_VERSION) embebidos.

Uso:  python tools/check_resources.py [ruta_al_exe]
"""

import os
import struct
import sys

RT_ICON = 3
RT_GROUP_ICON = 14
RT_VERSION = 16

NAMES = {RT_ICON: "RT_ICON", RT_GROUP_ICON: "RT_GROUP_ICON", RT_VERSION: "RT_VERSION"}


def load_rsrc(exe):
    """Devuelve {tipo: {nombre: [(lang, offset, size)]}} leyendo .rsrc."""
    if exe[:2] != b"MZ":
        raise ValueError("no es un PE valido (falta firma MZ)")
    pe = struct.unpack_from("<I", exe, 0x3C)[0]
    if exe[pe : pe + 4] != b"PE\x00\x00":
        raise ValueError("no es un PE valido (falta firma PE)")

    nsec = struct.unpack_from("<H", exe, pe + 6)[0]
    opt = pe + 24
    magic = struct.unpack_from("<H", exe, opt)[0]
    dd_off = opt + (112 if magic == 0x20B else 96)
    rsrc_rva, _ = struct.unpack_from("<II", exe, dd_off + 2 * 8)
    if rsrc_rva == 0:
        return {}

    # Tabla de secciones (40 bytes por entrada).
    sec_off = opt + struct.unpack_from("<H", exe, pe + 20)[0]
    sections = []
    for i in range(nsec):
        vsize, vaddr, rsize, rptr = struct.unpack_from(
            "<IIII", exe, sec_off + i * 40 + 8
        )
        sections.append((vaddr, max(vsize, rsize), rptr))
    sec = next(
        (s for s in sections if s[0] <= rsrc_rva < s[0] + s[1]), None
    )
    if sec is None:
        return {}
    base = sec[2] + (rsrc_rva - sec[0])

    # Los punteros del arbol de recursos son OFFSETS relativos al inicio de
    # la seccion .rsrc (no RVAs absolutos).
    def walk(off_rel, depth):
        off = base + off_rel
        n = struct.unpack_from("<H", exe, off + 12)[0]
        e = struct.unpack_from("<H", exe, off + 14)[0]
        out = {}
        for i in range(n + e):
            ident, child = struct.unpack_from("<II", exe, off + 16 + i * 8)
            key = ident if ident else child  # nombre numerico
            if depth < 2:
                out[key] = walk(child & 0x7FFFFFFF, depth + 1)
            else:
                # Entrada de datos: rva y tamano del blob del recurso.
                data_off = base + (child & 0x7FFFFFFF)
                data_rva, size = struct.unpack_from("<II", exe, data_off)
                out[key] = (i, base + data_rva, size)
        if depth == 1:
            # Aplana el nivel de idioma: por nombre -> lista de (lang, off, size).
            return {k: sorted(vals.values()) for k, vals in out.items()}
        return out

    return walk(0, 0)


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    exe_path = (
        sys.argv[1]
        if len(sys.argv) > 1
        else os.path.join(root, "target", "release", "zendesktop.exe")
    )
    if not os.path.isfile(exe_path):
        print(f"ERROR: no existe {exe_path}")
        return 1
    exe = open(exe_path, "rb").read()
    rsrc = load_rsrc(exe)

    if not rsrc:
        print("No hay tabla de recursos (.rsrc).")
        return 1

    total = 0
    for rtype in sorted(rsrc):
        label = NAMES.get(rtype, f"tipo {rtype}")
        for name in sorted(rsrc[rtype], key=lambda k: (k if isinstance(k, int) else 0)):
            for lang, off, size in sorted(rsrc[rtype][name]):
                total += 1
                print(
                    f"{label:16} id={name:<4} lang=0x{lang:04x} "
                    f"offset=0x{off:x} size={size}"
                )
    print(f"\nOK: {os.path.basename(exe_path)} con {total} recursos en {len(rsrc)} tipos.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
