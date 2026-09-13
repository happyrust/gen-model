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
originals = evidence / 'lazy-health-original'
entries, diff = [], []
for original in sorted(originals.rglob('*')):
    if not original.is_file():
        continue
    relative = original.relative_to(originals)
    modified = root / relative
    before, after = original.read_bytes(), modified.read_bytes()
    if before == after:
        continue
    entries.append({'path': relative.as_posix(),
                    'original_sha256': hashlib.sha256(before).hexdigest(),
                    'modified_sha256': hashlib.sha256(after).hexdigest()})
    diff.extend(difflib.unified_diff(before.decode('utf-8').splitlines(True),
                                     after.decode('utf-8').splitlines(True),
                                     fromfile='a/' + relative.as_posix(),
                                     tofile='b/' + relative.as_posix()))
(evidence / 'lazy-health-hashes.json').write_text(json.dumps(entries, indent=2), encoding='utf-8')
(evidence / 'lazy-health.patch').write_bytes(''.join(diff).encode('utf-8'))
with tempfile.TemporaryDirectory(prefix='lazy-health-rollback-') as temp:
    temp = Path(temp)
    for entry in entries:
        target = temp / entry['path']
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(root / entry['path'], target)
    verified = subprocess.run([sys.executable, str(evidence / 'lazy-health-rollback.py'),
                               '--root', str(temp)], capture_output=True, text=True, check=True)
    (evidence / 'lazy-health-rollback.log').write_text(verified.stdout, encoding='utf-8')
    print(verified.stdout.strip())
