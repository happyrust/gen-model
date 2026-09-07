"""Find native spec selections for catalogue instances without editing data."""
import json
import equi8000_live as e
folder=e.OUT/'catalogue-spco-probe';folder.mkdir(exist_ok=False)
mapping=json.loads((e.OUT/'remaining-catalogue-map/result.json').read_text())
names=sorted({r['candidates'][0]['name'] for r in mapping if r['candidates']})
commands=[]
for name in names:
    commands+=['$P SCOM-'+name,name,'Q REF','Q GMREF',
        f'VAR !items COLLECT ALL SPCO WITH ( CATR EQ {name} )',
        'DO !item VALUES !items',' $!item',' Q REF',' Q NAME',' Q CATR',' BREAK','ENDDO']
e.tty(folder,commands)
print((folder/'tty.log').read_text(encoding='utf-8-sig'))
