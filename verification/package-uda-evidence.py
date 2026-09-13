from pathlib import Path
import difflib
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parent.parent
evidence = root / 'verification'
originals = evidence / 'uda-provider-original'
added = ['src/data_interface/uda_catalog_provider.rs']
paths = [p.relative_to(originals).as_posix() for p in originals.rglob('*') if p.is_file()] + added
entries, diff = [], []
for relative in sorted(paths):
    original, modified = originals / relative, root / relative
    before = original.read_bytes() if original.exists() else b''
    after = modified.read_bytes()
    if before == after:
        continue
    entries.append({'path': relative,
        'original_sha256': hashlib.sha256(before).hexdigest() if original.exists() else None,
        'modified_sha256': hashlib.sha256(after).hexdigest()})
    diff.extend(difflib.unified_diff(before.decode('utf-8').splitlines(True),
        after.decode('utf-8').splitlines(True),
        fromfile='a/' + relative if original.exists() else '/dev/null', tofile='b/' + relative))
(evidence / 'uda-provider-hashes.json').write_text(json.dumps(entries, indent=2), encoding='utf-8')
(evidence / 'uda-provider.patch').write_bytes(''.join(diff).encode('utf-8'))
with tempfile.TemporaryDirectory(prefix='uda-rollback-') as temp:
    temp = Path(temp)
    for entry in entries:
        target = temp / entry['path']
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(root / entry['path'], target)
    checked = subprocess.run([sys.executable, str(evidence / 'uda-provider-rollback.py'),
                              '--root', str(temp)], capture_output=True, text=True, check=True)
    (evidence / 'uda-provider-rollback.log').write_text(checked.stdout, encoding='utf-8')
    print(checked.stdout.strip())
