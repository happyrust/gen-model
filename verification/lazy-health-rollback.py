"""Restore only this repair batch, refusing to overwrite any later source edits."""
from pathlib import Path
import argparse
import hashlib
import json

parser = argparse.ArgumentParser()
parser.add_argument('--root', type=Path, required=True)
args = parser.parse_args()
evidence = Path(__file__).resolve().parent
entries = json.loads((evidence / 'lazy-health-hashes.json').read_text(encoding='utf-8'))
for entry in entries:
    current = args.root / entry['path']
    original = evidence / 'lazy-health-original' / entry['path']
    assert hashlib.sha256(current.read_bytes()).hexdigest() == entry['modified_sha256'], current
    assert hashlib.sha256(original.read_bytes()).hexdigest() == entry['original_sha256'], original
for entry in entries:
    target = args.root / entry['path']
    target.write_bytes((evidence / 'lazy-health-original' / entry['path']).read_bytes())
    assert hashlib.sha256(target.read_bytes()).hexdigest() == entry['original_sha256']
print(f'rollback verified: {len(entries)} files')
