#!/usr/bin/env python3
"""Export the editable brand sources to Retina PNG, App Store PNG and ICNS."""
import copy
import io
import json
import xml.etree.ElementTree as ET
import pathlib
import subprocess
import tempfile
import cairosvg
from PIL import Image, ImageFilter, ImageChops, ImageDraw, ImageFont

ROOT = pathlib.Path(__file__).resolve().parent.parent
BRAND = ROOT / 'assets/branding'
# Render the editable vectors at 2x, then composite soft material shadows.
# Separate layers keep shadows reproducible without SVG filter support in Cairo.
source = ET.fromstring((BRAND / 'AppIcon.svg').read_text())

def layer(identifier):
    root = copy.deepcopy(source)
    for child in list(root):
        if child.tag.endswith('}g') and child.get('id') != identifier:
            root.remove(child)
    return Image.open(io.BytesIO(cairosvg.svg2png(bytestring=ET.tostring(root),
        output_width=2048, output_height=2048))).convert('RGBA')

def shadow(im, radius, opacity, offset, color):
    alpha = im.getchannel('A').filter(ImageFilter.GaussianBlur(radius))
    alpha = ImageChops.offset(alpha, 0, offset).point(lambda a: round(a * opacity))
    result = Image.new('RGBA', im.size, color)
    result.putalpha(alpha)
    return result

tile, symbol = layer('tile'), layer('symbol')
master = shadow(tile, 14, .16, 14, '#253354')
master.alpha_composite(tile)
master.alpha_composite(shadow(symbol, 42, .23, 36, '#213D86'))
master.alpha_composite(symbol)
master = master.resize((1024, 1024), Image.Resampling.LANCZOS)
master.save(BRAND / 'AppIcon.png')
# A separate opaque, square store master has no baked-in outer rounded mask.
# The macOS ICNS / asset catalog retain the standard optical inset and alpha.
store_root = copy.deepcopy(source)
for child in list(store_root):
    if child.get('id') == 'tile':
        store_root.remove(child)
ns = '{http://www.w3.org/2000/svg}'
store_root.insert(1, ET.Element(ns + 'rect', {'width': '1024', 'height': '1024', 'fill': 'url(#shell)'}))
store = Image.open(io.BytesIO(cairosvg.svg2png(bytestring=ET.tostring(store_root),
    output_width=2048, output_height=2048))).convert('RGB')
store.resize((1024, 1024), Image.Resampling.LANCZOS).save(BRAND / 'AppIcon-AppStore-1024.png')
assetset = BRAND / 'AppIcon.appiconset'
assetset.mkdir(exist_ok=True)
images = []
with tempfile.TemporaryDirectory() as td:
    iconset = pathlib.Path(td) / 'AppIcon.iconset'
    iconset.mkdir()
    for point in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            suffix = '@2x' if scale == 2 else ''
            name = f'icon_{point}x{point}{suffix}.png'
            sized = master.resize((point * scale, point * scale), Image.Resampling.LANCZOS)
            sized.save(iconset / name)
            sized.save(assetset / name)
            images.append({'idiom': 'mac', 'size': f'{point}x{point}',
                           'scale': f'{scale}x', 'filename': name})
    subprocess.run(['iconutil', '-c', 'icns', str(iconset), '-o', str(BRAND / 'AppIcon.icns')], check=True)
(assetset / 'Contents.json').write_text(json.dumps({'images': images,
    'info': {'author': 'xcode', 'version': 1}}, indent=2) + '\n')
# Keep the review sheet synchronized with the actual packaged icon.
preview = Image.new('RGB', (1440, 900), '#F3F5FA')
draw = ImageDraw.Draw(preview)
font_path = '/System/Library/Fonts/Helvetica.ttc'
def font(size):
    return ImageFont.truetype(font_path, size)
draw.text((78, 58), 'Removent', font=font(36), fill='#202946')
draw.text((80, 111), 'APP ICON / REMOTE DESKTOP', font=font(14), fill='#6B748C')
hero = master.resize((620, 620), Image.Resampling.LANCZOS)
preview.paste(hero, (42, 173), hero)
for top, fill, label in [(201, '#FFFFFF', '#7A8297'), (482, '#182036', '#A3ACC4')]:
    draw.rounded_rectangle((744, top, 1360, top + 254), radius=30, fill=fill)
    draw.text((774, top + 23), '128 / 64 / 32 / 16 PX', font=font(14), fill=label)
    for x, size in [(792, 128), (998, 64), (1157, 32), (1270, 16)]:
        icon = master.resize((size, size), Image.Resampling.LANCZOS)
        preview.paste(icon, (x, top + 77 + (128 - size) // 2), icon)
preview.save(BRAND / 'AppIcon-preview.png')
background = Image.open(io.BytesIO(cairosvg.svg2png(url=str(BRAND / 'dmg-background.svg')))).convert('RGB')
background.save(BRAND / 'dmg-background@2x.png', dpi=(144, 144))
with tempfile.TemporaryDirectory() as td:
    small = pathlib.Path(td) / 'background.png'
    background.resize((700, 450), Image.Resampling.LANCZOS).save(small, dpi=(72, 72))
    subprocess.run(['tiffutil', '-cathidpicheck', str(small), str(BRAND / 'dmg-background@2x.png'),
                    '-out', str(BRAND / 'dmg-background.tiff')], check=True)
print('Brand assets exported to', BRAND)
