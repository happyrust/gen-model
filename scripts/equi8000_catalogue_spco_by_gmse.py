"""Find spec selections via geometry-set reference rather than one nominal size."""
import json
import equi8000_live as e
folder=e.OUT/'catalogue-spco-by-gmse';folder.mkdir(exist_ok=False)
mapping=json.loads((e.OUT/'remaining-catalogue-map/result.json').read_text())
refs=sorted({r['gmse'] for r in mapping if r['gmse']})
commands=[]
for ref in refs:
    commands+=['$P GMSE-'+ref,
        f'VAR !items COLLECT ALL SPCO WITH ( GMRE OF CATR EQ ={ref} )',
        'DO !item VALUES !items',' $!item',' Q REF',' Q NAME',' Q CATR',' BREAK','ENDDO']
e.tty(folder,commands)
print((folder/'tty.log').read_text(encoding='utf-8-sig'))
