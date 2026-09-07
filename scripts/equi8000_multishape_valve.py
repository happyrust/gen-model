"""Private native VALV covering SCYL, LSNO, SBOX, SCON and SLINE."""
import sys
import equi8000_live as e

phase = sys.argv[1]
folder = e.OUT / ('multishape-valve-edit-' + phase)
folder.mkdir(exist_ok=False)
if phase == 'create':
    assert not e.sql("SELECT VALUE id FROM pe WHERE name='/CODEX_TYPES_MULTI_BRAN' AND deleted!=true;")[0]
    commands = ['/CODEX_TYPES_CAT_PIPE', 'NEW BRAN /CODEX_TYPES_MULTI_BRAN',
                'HPOS E 5000 N 0 U 0', 'TPOS E 5000 N 0 U 0', 'Q REF',
                'NEW VALV /CODEX_TYPES_MULTI_VALV', 'SPRE /NAC-B/A0Ph33VNN0-D:25x25',
                'POS E 5000 N 0 U 0', 'ORI Y IS N AND Z IS U']
elif phase == 'move':
    commands = ['/CODEX_TYPES_MULTI_VALV', 'BY N 700', 'SHOP TRUE', 'BUIL TRUE']
elif phase == 'move-fixed':
    commands = ['/CODEX_TYPES_MULTI_VALV', 'BY N 700', 'SHOP FALSE', 'BUIL FALSE']
else:
    raise ValueError(phase)
e.tty(folder, commands + ['Q REF', 'Q NAME', 'Q POS', 'Q ORI', 'Q SPRE', 'Q SHOP', 'Q BUIL',
                           "SAVEWORK 'Multishape valve " + phase + "'"])
print((folder / 'tty.log').read_text(encoding='utf-8-sig'))
