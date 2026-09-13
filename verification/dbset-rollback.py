from pathlib import Path
import argparse
import hashlib
import json

parser = argparse.ArgumentParser()
parser.add_argument('--root', type=Path, required=True, help='e3d-io checkout or disposable copy')
args = parser.parse_args()
root = args.root.resolve()
evidence = Path(__file__).resolve().parent
rows = json.loads((evidence / 'dbset-hashes.json').read_text(encoding='utf-8'))
for row in rows:
    path = (root / row['path']).resolve()
    assert path.is_relative_to(root), path
    assert hashlib.sha256(path.read_bytes()).hexdigest() == row['modified_sha256'], path
    if row['original']:
        assert hashlib.sha256((evidence / row['original']).read_bytes()).hexdigest() == row['original_sha256']
for row in rows:
    path = root / row['path']
    if row['original']:
        path.write_bytes((evidence / row['original']).read_bytes())
        assert hashlib.sha256(path.read_bytes()).hexdigest() == row['original_sha256']
    else:
        path.unlink()
        assert not path.exists()
print(f'rollback verified: {len(rows)} files')
