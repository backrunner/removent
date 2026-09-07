# Removent icon

The icon depicts two interlocking display-shaped frames, representing the
connection between local and remote devices. Blue-violet and aqua surfaces use
fine bevels and a soft shadow on a pale macOS tile. The mark contains no letters,
cursors, arrows or decorative waves. Its crossing is drawn explicitly so the
frames interlock rather than merely overlap.

Sources and deliverables:

- `AppIcon.svg`: editable vector source; tile and symbol are separate groups.
- `AppIcon.png`: 1024 × 1024 RGBA macOS icon, with optical inset and transparent exterior.
- `AppIcon.icns`: 16, 32, 128, 256 and 512 point slots at 1× and 2×.
- `AppIcon.appiconset/`: the same ten macOS slots with Xcode Contents.json.
- `AppIcon-AppStore-1024.png`: opaque square RGB master without a baked outer mask.
- `AppIcon-preview.png`: appearance on light/dark surfaces and at smaller sizes.

`python3 scripts/branding.py` reproducibly renders SVG layers at 2048px, adds soft
shadows, then downsamples to delivery sizes. CairoSVG, Pillow and Apple's iconutil
are required. No image generation API was used.

Checked against Apple's Xcode guidance: macOS requires assets for each size.
The asset catalog is validated with `actool --platform macosx`; ICNS is decoded
with iconutil to check the packaged representation. These checks validate asset
format, not App Store approval of the application or predicted conversion rates.

Reference: https://developer.apple.com/documentation/xcode/configuring-your-app-icon
