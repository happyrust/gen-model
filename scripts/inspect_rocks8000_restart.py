import json,sys
from rocksdb8000_evidence import RUN
folder=RUN/sys.argv[1]
a=json.loads((folder/'before-manifest.json').read_text())
b=json.loads((folder/'after-manifest.json').read_text())
for table in sorted(set(a)|set(b)):
    if a.get(table)==b.get(table):continue
    print('TABLE',table,a.get(table),b.get(table))
    x=json.loads((folder/f'before-{table}.json').read_text())
    y=json.loads((folder/f'after-{table}.json').read_text())
    xm={r['id']:r for r in x};ym={r['id']:r for r in y}
    changes=[]
    for key in sorted(set(xm)|set(ym)):
        if xm.get(key)==ym.get(key):continue
        old,new=xm.get(key,{}),ym.get(key,{})
        changes.append(dict(id=key,fields={k:[old.get(k),new.get(k)] for k in set(old)|set(new) if old.get(k)!=new.get(k)}))
    print(json.dumps(changes[:4],ensure_ascii=False))
