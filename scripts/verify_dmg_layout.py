#!/usr/bin/env python3
"""Check the mounted installer, including Finder metadata and Retina artwork."""
import pathlib
import sys

from ds_store import DSStore
from mac_alias import Alias
from PIL import Image


def verify(mount):
    mount = pathlib.Path(mount)
    assert (mount / 'Removent.app/Contents/MacOS/removent-launcher').is_file()
    assert (mount / 'Applications').is_symlink()
    assert (mount / 'Applications').readlink() == pathlib.Path('/Applications')
    with DSStore.open(str(mount / '.DS_Store'), 'r') as db:
        assert db['.']['vstl'] == (b'type', b'icnv'), 'Installer must open in icon view'
        assert db['.']['icvl'] == (b'type', b'icnv')
        assert db['.']['bwsp']['WindowBounds'] == '{{200, 160}, {700, 450}}'
        options = db['.']['icvp']
        assert options['backgroundType'] == 2
        alias = Alias.from_bytes(options['backgroundImageAlias'])
        assert alias.target.filename == 'background.tiff'
        assert alias.target.posix_path == '/.background/background.tiff', alias.target.posix_path
        assert alias.target.cnid == (mount / '.background/background.tiff').stat().st_ino
        assert db['Removent.app']['Iloc'] == (220, 245)
        assert db['Applications']['Iloc'] == (480, 245)
    with Image.open(mount / '.background/background.tiff') as artwork:
        sizes = set()
        for index in range(artwork.n_frames):
            artwork.seek(index)
            sizes.add(artwork.size)
        assert sizes == {(700, 450), (1400, 900)}, sizes
    print('Installer layout and Retina background verified')


if __name__ == '__main__':
    verify(sys.argv[1])
