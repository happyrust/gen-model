"""Read E3D's existing civil ownership and geometry hierarchy without saving."""
import sys
import equi8000_live as e
detail=len(sys.argv)>1
folder=e.OUT/('civil-source-detail' if detail else 'civil-source-hierarchy')
folder.mkdir(exist_ok=False)
commands=[]
for noun in ['FLOOR','SCREED','GWALL']:
    commands += [f'$P CIVIL-{noun}',f'VAR !items COLLECT ALL {noun}',
        'DO !item VALUES !items',' $!item',' Q REF',' Q TYPE',' Q OWNER',' Q MEMB',
        ' BREAK','ENDDO']
if detail:
    commands=[]
    for ref in ['17496/72443','17496/105691']:
        commands += ['='+ref,'Q ATT','OWNER','Q REF','Q TYPE','Q ATT',
            'OWNER','Q REF','Q TYPE','Q OWNER','Q MEMB']
e.tty(folder,commands)
print((folder/'tty.log').read_text(encoding='utf-8-sig',errors='replace'))
