"""Read native catalogue types and geometry children for usable selections."""
import sys
import equi8000_live as e

folder = e.OUT / ('remaining-spec-native-detail' + ('-' + sys.argv[1] if len(sys.argv) > 1 else ''))
folder.mkdir(exist_ok=False)
commands = []
for ref in ('13245/643605', '15192/236365'):
    commands += ['$P SPEC-' + ref, '=' + ref, 'Q REF', 'Q NAME', 'Q CATR']
if len(sys.argv) > 1 and sys.argv[1] == 'components':
    commands = []
    for name in ('/A0Ph33VNN0FF', '/MODELLED.WELD.SHOP.FF10'):
        commands += [name, 'Q REF', 'Q GTYP', 'Q GMRE', 'Q PARA']
    for ref in ('13245/524827', '15192/53965'):
        commands += ['=' + ref, 'Q REF', 'Q MEMB']
elif len(sys.argv) > 1 and sys.argv[1] == 'srev':
    commands = [
        '=15192/245830', 'Q REF', 'Q NAME', 'Q CATR',
        '/MACB200CC', 'Q REF', 'Q GTYP', 'Q GMRE', 'Q PARA',
        '=15192/225911', 'Q REF', 'Q MEMB',
    ]
elif len(sys.argv) > 1 and sys.argv[1] == 'spco-path':
    commands = [
        '=13244/108601', 'Q REF', 'Q NAME', 'Q CATR',
        'CATR', 'Q REF', 'Q GTYP', 'Q GMRE',
        'GMRE', 'Q REF', 'Q MEMB',
    ]
e.tty(folder, commands)
print((folder / 'tty.log').read_text(encoding='utf-8-sig'))
