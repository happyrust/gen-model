"""Read native implicit connection points for both fixture component nouns."""
import equi8000_live as e
folder=e.OUT/'catalogue-points-probe';folder.mkdir(exist_ok=False)
commands=[]
for name in ['/CODEX_TYPES_CAT_SREV_UPDATED','/CODEX_TYPES_COUP_SREV']:
    commands += [name,'Q REF','Q APOS','Q LPOS','Q ADIR','Q LDIR','Q ABOR','Q LBOR']
e.tty(folder,commands)
print((folder/'tty.log').read_text(encoding='utf-8-sig'))
