"""Ask native E3D for any available volume/area measures on the broad valve."""
import equi8000_live as e


folder = e.OUT / 'remaining-broad-native-measures'
folder.mkdir(exist_ok=False)
e.tty(folder, [
    '/CODEX_TYPES_REMAIN_BROAD_VALV',
    'Q VOLUME',
    'Q VOLM',
    'Q AREA',
    'Q ATTS',
])
print((folder / 'tty.log').read_text(encoding='utf-8-sig'))
