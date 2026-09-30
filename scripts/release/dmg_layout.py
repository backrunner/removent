#!/usr/bin/env python3
"""Write Finder window layout without Apple Events or an interactive desktop."""
import pathlib
import sys
from ds_store import DSStore
from mac_alias import Alias

# mac_alias computes a volume-relative path. /tmp is a symlink to /private/tmp;
# passing the unresolved spelling produces an alias that escapes the DMG volume
# and points at the builder's temporary folder on the destination Mac.
mount = pathlib.Path(sys.argv[1]).resolve()
background = mount / '.background' / 'background.tiff'
with DSStore.open(str(mount / '.DS_Store'), 'w+') as db:
    db['.']['bwsp'] = {'ShowStatusBar': False, 'ShowToolbar': False, 'ShowTabView': False,
        'ShowPathbar': False, 'ShowSidebar': False, 'SidebarWidth': 0,
        'ContainerShowSidebar': False, 'WindowBounds': '{{200, 160}, {700, 450}}'}
    db['.']['icvp'] = {'viewOptionsVersion': 1, 'backgroundType': 2,
        'backgroundColorRed': 1.0, 'backgroundColorGreen': 1.0, 'backgroundColorBlue': 1.0,
        'scrollPositionX': 0.0, 'scrollPositionY': 0.0,
        'backgroundImageAlias': Alias.for_file(str(background)).to_bytes(),
        'iconSize': 100.0, 'gridOffsetX': 0.0, 'gridOffsetY': 0.0,
        'gridSpacing': 100.0, 'arrangeBy': 'none', 'showIconPreview': True,
        'showItemInfo': False, 'labelOnBottom': True, 'textSize': 12.0}
    db['.']['vSrn'] = ('long', 1)
    # Store icon view as both the default and current view; list/column views
    # do not display the installer background.
    db['.']['icvl'] = ('type', b'icnv')
    db['.']['vstl'] = ('type', b'icnv')
    db['Removent.app']['Iloc'] = (220, 245)
    db['Applications']['Iloc'] = (480, 245)
