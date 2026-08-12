from PIL import Image, ImageOps

for name in ["general", "rules", "appearance"]:
    src = f"target/settings_probe_{name}.bmp"
    im = Image.open(src).convert("RGBA")
    im = ImageOps.flip(im)  # filas GDI bottom-up
    out = f"assets/icons/settings_probe_{name}.png"
    im.save(out)
    print(name, im.size)
