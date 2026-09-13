from pathlib import Path
import argparse
import hashlib
import json

parser = argparse.ArgumentParser()
parser.add_argument('--root', type=Path, required=True)
args = parser.parse_args()
root = args.root.resolve()
evidence = Path(__file__).resolve().parent
entries = json.loads((evidence / 'uda-provider-hashes.json').read_text(encoding='utf-8'))
for entry in entries:
    target = (root / entry['path']).resolve()
    assert target.is_relative_to(root), target
    assert hashlib.sha256(target.read_bytes()).hexdigest() == entry['modified_sha256'], target
    if entry['original_sha256'] is not None:
        original = evidence / 'uda-provider-original' / entry['path']
        assert hashlib.sha256(original.read_bytes()).hexdigest() == entry['original_sha256'], original
for entry in entries:
    target = root / entry['path']
    if entry['original_sha256'] is None:
        target.unlink()
        assert not target.exists()
    else:
        target.write_bytes((evidence / 'uda-provider-original' / entry['path']).read_bytes())
        assert hashlib.sha256(target.read_bytes()).hexdigest() == entry['original_sha256']
print(f'rollback verified: {len(entries)} files')
