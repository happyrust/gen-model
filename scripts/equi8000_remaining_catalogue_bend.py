"""Clone and increment a native duct BEND covering LINE, SEXT and SREV."""
import sys

import equi8000_live as e


phase = sys.argv[1]
folder = e.OUT / ('remaining-bend-edit-' + phase)
folder.mkdir(exist_ok=False)

branch = '/CODEX_TYPES_REMAIN_BEND'
member = '/CODEX_TYPES_REMAIN_BEND_SREV'
if phase == 'create':
    assert not e.sql(
        f"SELECT VALUE id FROM pe WHERE name='{branch}' AND deleted!=true;"
    )[0]
    commands = [
        '/CODEX_TYPES_CAT_PIPE',
        f'NEW BRAN {branch} COPY /C-OR-1R332-B',
        'VAR !items COLLECT ALL BEND WITH ( SPRE EQ =13244/108845 ) FOR CE',
        'DO !item VALUES !items',
        ' $!item',
        f' NAME {member}',
        ' BREAK',
        'ENDDO',
    ]
elif phase == 'move':
    commands = [member, 'BY U 350', 'SHOP TRUE', 'BUIL TRUE']
elif phase == 'restore-flags':
    commands = [member, 'SHOP FALSE', 'BUIL FALSE']
else:
    raise ValueError(phase)

e.tty(
    folder,
    commands
    + [
        'Q REF',
        'Q NAME',
        'Q POS',
        'Q ORI',
        'Q SPRE',
        'Q SHOP',
        'Q BUIL',
        f"SAVEWORK 'Remaining catalogue bend {phase}'",
    ],
)
print((folder / 'tty.log').read_text(encoding='utf-8-sig'))
