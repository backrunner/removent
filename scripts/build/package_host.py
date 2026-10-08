#!/usr/bin/env python3
"""Give the capture process its own stable app identity for macOS TCC."""
from pathlib import Path
import plistlib
import shutil
import sys

app, executable, version, build = sys.argv[1:]
app = Path(app)
host = app / 'Contents/Helpers/RemoventHost.app'
(host / 'Contents/MacOS').mkdir(parents=True)
(host / 'Contents/Resources/zh-Hans.lproj').mkdir(parents=True)
shutil.copy2(executable, host / 'Contents/MacOS/removentd')
shutil.copy2('assets/branding/AppIcon.icns', host / 'Contents/Resources/AppIcon.icns')
(host / 'Contents/Info.plist').write_bytes(plistlib.dumps({
    'CFBundleIdentifier': 'com.alkinum.removent.host',
    'CFBundleExecutable': 'removentd',
    'CFBundleName': 'Removent Host',
    'CFBundleDisplayName': 'Removent Host',
    'CFBundlePackageType': 'APPL',
    'CFBundleVersion': build,
    'CFBundleShortVersionString': version,
    'CFBundleIconFile': 'AppIcon',
    'CFBundleDevelopmentRegion': 'en',
    'CFBundleLocalizations': ['en', 'zh-Hans'],
    'LSMinimumSystemVersion': '26.0',
    'LSUIElement': True,
    'NSScreenCaptureUsageDescription': 'Share your screen with devices you approve.',
    'NSAccessibilityUsageDescription': 'Let approved devices control this Mac.',
}))
(host / 'Contents/Resources/zh-Hans.lproj/InfoPlist.strings').write_text(
    '"CFBundleDisplayName" = "Removent 屏幕共享";\n'
    '"NSScreenCaptureUsageDescription" = "向你批准的设备共享本机屏幕。";\n'
    '"NSAccessibilityUsageDescription" = "允许你批准的设备控制本机。";\n')
# Preserve the CLI/desktop's adjacent-executable lookup. Service::plist
# canonicalizes this path, so launchd runs the actual helper-bundle executable.
(app / 'Contents/MacOS/removentd').symlink_to('../Helpers/RemoventHost.app/Contents/MacOS/removentd')
