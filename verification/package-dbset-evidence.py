from pathlib import Path
import difflib
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile

evidence = Path(__file__).resolve().parent
vendor = evidence.parent.parent / 'vendor/e3d-io'
rows, diff = [], []
for relative, original in [('src/db_element.rs', 'dbset-original/db_element.rs'),
                           ('tests/dbset_generations.rs', None)]:
    before = (evidence / original).read_bytes() if original else b''
    after = (vendor / relative).read_bytes()
    rows.append({'path': relative, 'original': original,
        'original_sha256': hashlib.sha256(before).hexdigest() if original else None,
        'modified_sha256': hashlib.sha256(after).hexdigest()})
    diff.extend(difflib.unified_diff(before.decode('utf-8').splitlines(True),
        after.decode('utf-8').splitlines(True), fromfile='a/' + relative if original else '/dev/null',
        tofile='b/' + relative))
(evidence / 'dbset-hashes.json').write_text(json.dumps(rows, indent=2), encoding='utf-8')
(evidence / 'dbset.patch').write_bytes(''.join(diff).encode('utf-8'))
with tempfile.TemporaryDirectory(prefix='dbset-rollback-') as temp:
    temp = Path(temp)
    for row in rows:
        path = temp / row['path']
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(vendor / row['path'], path)
    checked = subprocess.run([sys.executable, str(evidence / 'dbset-rollback.py'), '--root', str(temp)],
                             capture_output=True, text=True, check=True)
    (evidence / 'dbset-rollback.log').write_text(checked.stdout, encoding='utf-8')
    print(checked.stdout.strip())
