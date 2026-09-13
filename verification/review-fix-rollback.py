from pathlib import Path
import argparse, hashlib, json, shutil
parser=argparse.ArgumentParser()
parser.add_argument('--root',type=Path,required=True)
args=parser.parse_args()
base=Path(__file__).resolve().parent
items=json.loads((base/'review-fix-hashes.json').read_text())
for item in items:
    target=args.root/item['path']
    if hashlib.sha256(target.read_bytes()).hexdigest()!=item['after']:
        raise RuntimeError('Subsequent edits detected: '+str(target))
for item in items:
    target=args.root/item['path']
    shutil.copy2(base/'review-fix-original'/item['path'],target)
    assert hashlib.sha256(target.read_bytes()).hexdigest()==item['before']
print('rollback verified')
