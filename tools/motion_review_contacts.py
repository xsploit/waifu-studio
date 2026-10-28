"""Upper-body contact sheets for triage, not full-motion acceptance.

Six exact frames across each completed native quad capture; four views per frame.
Lossless original frames remain available for the full review and lower body.
Requires Pillow. The heading explicitly identifies sparse sampling.
"""
import argparse
import json
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont


def build(root):
    out = root / "contacts"
    out.mkdir(exist_ok=True)
    try:
        font = ImageFont.truetype("C:/Windows/Fonts/arial.ttf", 17)
    except OSError:
        font = ImageFont.load_default(size=17)
    count = 0
    for receipt in sorted((root / "frames").glob("*/receipt.json")):
        directory = receipt.parent
        target = out / f"{directory.name}.jpg"
        if target.exists():
            continue
        data = json.loads(receipt.read_text(encoding="utf-8"))
        n = data["captured_frames"]
        samples = sorted(set(round((n-1)*i/5) for i in range(6)))
        sheet = Image.new("RGB", (len(samples)*220, 1135), "#25282e")
        draw = ImageDraw.Draw(sheet)
        draw.text((10, 8), f"{directory.name} · TRIAGE: 6 sampled frames, not a full review", font=font, fill="white")
        for column, frame in enumerate(samples):
            draw.text((column*220+8, 35), f"frame {frame} · {frame/30:.3f}s", font=font, fill="white")
            image = Image.open(directory / f"frame_{frame:04}.png").convert("RGBA")
            for row, (x,y) in enumerate([(0,0),(640,0),(0,480),(640,480)]):
                crop = image.crop((x+210, y+25, x+430, y+290))
                sheet.paste(crop, (column*220, 65+row*265), crop)
        sheet.save(target, quality=95)
        count += 1
    print(json.dumps({"new_triage_sheets":count, "scope":"6 upper-body samples per clip; no automatic acceptance"}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    build(parser.parse_args().root)
