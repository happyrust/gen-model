"""Clone and increment legal AMS8000 catalogue cases for SCTO and SBOX."""
import sys

import equi8000_live as e


CASES = {
    'scto': {
        'source': '/C-OR-1R332-H',
        'branch': '/CODEX_TYPES_REMAIN_SCTO',
        'noun': 'BEND',
        'member': '/CODEX_TYPES_REMAIN_SCTO_BEND',
        'spre': '13244/109849',
        'move': 'BY E 275',
    },
    'sbox': {
        'source': '/C-GR-1RX-3000A',
        'branch': '/CODEX_TYPES_REMAIN_SBOX',
        'noun': 'ATTA',
        'member': '/CODEX_TYPES_REMAIN_SBOX_ATTA',
        'spre': '13244/108749',
        'move': 'BY N 225',
    },
    'broad': {
        'source': '=24384/26229',
        'branch': '/CODEX_TYPES_REMAIN_BROAD',
        'noun': 'VALV',
        'member': '/CODEX_TYPES_REMAIN_BROAD_VALV',
        'spre': '13245/851590',
        'move': 'BY U 325',
    },
    'redu': {
        'source': '=24384/24676',
        'branch': '/CODEX_TYPES_REMAIN_REDU',
        'noun': 'REDU',
        'member': '/CODEX_TYPES_REMAIN_REDU_TARGET',
        'spre': '13244/109009',
        'move': 'BY U 300',
    },
}

case_name, phase = sys.argv[1:3]
case = CASES[case_name]
folder = e.OUT / f'remaining-{case_name}-edit-{phase}'
folder.mkdir(exist_ok=False)

if phase == 'create':
    assert not e.sql(
        f"SELECT VALUE id FROM pe WHERE name='{case['branch']}' AND deleted!=true;"
    )[0]
    commands = [
        '/CODEX_TYPES_CAT_PIPE',
        f"NEW BRAN {case['branch']} COPY {case['source']}",
        f"VAR !items COLLECT ALL {case['noun']} WITH ( SPRE EQ ={case['spre']} ) FOR CE",
        'DO !item VALUES !items',
        ' $!item',
        f" NAME {case['member']}",
        ' BREAK',
        'ENDDO',
    ]
elif phase == 'move':
    commands = [case['member'], case['move'], 'SHOP TRUE', 'BUIL TRUE']
elif phase == 'move-back':
    commands = [case['member'], 'BY D 325']
elif phase == 'move-again':
    commands = [case['member'], 'BY U 325']
elif phase == 'restore-flags':
    commands = [case['member'], 'SHOP FALSE', 'BUIL FALSE']
else:
    raise ValueError(phase)

e.tty(
    folder,
    commands
    + [
        case['member'],
        'Q REF',
        'Q NAME',
        'Q POS',
        'Q ORI',
        'Q SPRE',
        'Q SHOP',
        'Q BUIL',
        f"SAVEWORK 'Remaining catalogue {case_name} {phase}'",
    ],
)
print((folder / 'tty.log').read_text(encoding='utf-8-sig'))
