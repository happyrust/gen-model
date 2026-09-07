"""Read-only evidence capture for the isolated AMS 8000 RocksDB run."""
import base64
import hashlib
import json
import sys
import urllib.request
from pathlib import Path

RUN = Path(__file__).resolve().parents[1] / 'output/rocksdb-8000-20260905'

def sql(query):
    request = urllib.request.Request('http://127.0.0.1:18049/sql', query.encode(), headers={
        'Authorization': 'Basic ' + base64.b64encode(b'root:root').decode(),
        'Surreal-NS': 'rocks8000_20260905', 'Surreal-DB': 'AvevaMarineSample',
        'Accept': 'application/json', 'Content-Type': 'text/plain',
    })
    with urllib.request.urlopen(request, timeout=120) as response:
        result = json.load(response)
    for row in result:
        if row['status'] != 'OK':
            raise RuntimeError(row)
    return [row['result'] for row in result]

if __name__ == '__main__':
    action = sys.argv[1]
    if action == 'status':
        print(json.dumps(sql("SELECT * FROM dbnum_watermark WHERE dbnum=8000; SELECT id, noun, name FROM pe WHERE dbnum=8000 LIMIT 6; SELECT * FROM data_face_signature;"), ensure_ascii=False))
    elif action == 'generate':
        refno = sys.argv[2] if len(sys.argv) > 2 else '24384/22402'
        body = json.dumps({'refno':refno,'project':'AvevaMarineSample','mdb':'ALL','force':True}).encode()
        request = urllib.request.Request('http://127.0.0.1:18050/api/v1/model/ensure',body,headers={'Content-Type':'application/json'})
        with urllib.request.urlopen(request,timeout=150) as response:
            result = json.load(response)
        (RUN/('model-ensure-'+refno.replace('/','_')+'.json')).write_text(json.dumps(result,indent=2,ensure_ascii=False),encoding='utf-8')
        print(json.dumps(result,ensure_ascii=False))
    elif action == 'roots':
        print(json.dumps(sql("SELECT id, noun, name FROM pe WHERE dbnum=8000 AND noun='EQUI' LIMIT 5;"), ensure_ascii=False))
    elif action == 'report':
        before = json.loads((RUN/'before-manifest.json').read_text())
        after = json.loads((RUN/'after-manifest.json').read_text())
        assert before == after
        pe = json.loads((RUN/'after-pe.json').read_text())
        pe8000 = [row for row in pe if row.get('dbnum') == 8000]
        assert len(pe8000) == 6605
        watermarks = json.loads((RUN/'after-dbnum_watermark.json').read_text())
        watermark = next(row for row in watermarks if row.get('dbnum') == 8000)
        assert watermark['applied_sesno'] == 285
        branch = json.loads((RUN/'after-BRAN.json').read_text())
        # Unnamed source elements may omit NAME; geometric attributes must still survive.
        assert all('OWNER' in row and 'HBOR' in row and 'HPOS' in row for row in branch)
        assert any(row.get('NAME') for row in branch)
        counts = {t:after[t]['count'] for t in ['BRAN','EQUI','inst_relate','tubi_relate','inst_geo','aabb','trans']}
        summary = {'passed':True,'dbnum':8000,'session':285,'elements':len(pe8000),
                   'tables':len(after),'rows':sum(v['count'] for v in after.values()),'counts':counts,
                   'comparison':'row counts and canonical full-row SHA-256 identical after process restart'}
        (RUN/'result.json').write_text(json.dumps(summary,indent=2),encoding='utf-8')
        print(json.dumps(summary,indent=2))
    elif action in ('before','after'):
        if len(sys.argv)>2:
            RUN = RUN / sys.argv[2]
            RUN.mkdir(parents=True,exist_ok=True)
        assert not (RUN/f'{action}-manifest.json').exists(), 'Evidence already exists; choose a new run directory'
        tables = sql('INFO FOR DB;')[0]['tables']
        selected = sorted(tables)
        evidence = {}
        for table in selected:
            rows = sql(f'SELECT * FROM {table};')[0]
            rows.sort(key=lambda row: json.dumps(row,sort_keys=True,ensure_ascii=False))
            encoded = json.dumps(rows, sort_keys=True, ensure_ascii=False, separators=(',',':')).encode()
            evidence[table] = {'count':len(rows),'sha256':hashlib.sha256(encoded).hexdigest()}
            (RUN / f'{action}-{table}.json').write_bytes(encoded)
        (RUN / f'{action}-manifest.json').write_text(json.dumps(evidence,indent=2),encoding='utf-8')
        print(json.dumps({'tables':len(evidence),'rows':sum(v['count'] for v in evidence.values()),'nonempty':{k:v['count'] for k,v in evidence.items() if v['count']}},indent=2))
        if action == 'after':
            before = json.loads((RUN/'before-manifest.json').read_text())
            assert evidence == before, 'Restart changed captured data'
            assert evidence['inst_relate']['count'] > 0, 'No models captured'
            assert evidence['BRAN']['count'] > 0, 'No branch attributes captured'
            print('PASS: every captured table has identical row count and content hash after restart')
