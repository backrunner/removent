#!/usr/bin/env python3
"""Write Finder window layout without Apple Events or an interactive desktop."""
import pathlib
import sys
from ds_store import DSStore
from mac_alias import Alias

mount = pathlib.Path(sys.argv[1])
background = mount / '.background' / 'background.tiff'
with DSStore.open(str(mount / '.DS_Store'), 'w+') as db:
    db['.']['bwsp'] = {'ShowStatusBar': False, 'ShowToolbar': False,
        'ShowPathbar': False, 'ShowSidebar': False, 'SidebarWidth': 0,
        'ContainerShowSidebar': False, 'WindowBounds': '{{200, 160}, {700, 450}}'}
    db['.']['icvp'] = {'viewOptionsVersion': 1, 'backgroundType': 2,
        'backgroundImageAlias': Alias.for_file(str(background)).to_bytes(),
        'iconSize': 100.0, 'gridOffsetX': 0.0, 'gridOffsetY': 0.0,
        'gridSpacing': 100.0, 'arrangeBy': 'none', 'showIconPreview': True,
        'showItemInfo': False, 'labelOnBottom': True, 'textSize': 12.0}
    db['.']['vSrn'] = ('long', 1)
    db['.']['icvl'] = ('type', b'icnv')
    db['Removent.app']['Iloc'] = (220, 245)
    db['Applications']['Iloc'] = (480, 245)
