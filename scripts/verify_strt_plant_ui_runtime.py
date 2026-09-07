"""Check the real backend records and mesh files used by plant-ui."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import hashlib
import json
import requests

RUN = Path(__file__).resolve().parents[1] / 'output/strt-cpv-20260905/live'
BASE = 'http://127.0.0.1:18206'


def records(root):
    response = requests.post(BASE + '/api/v1/model/records', json={
        'generation_root': root, 'limit': 5000,
    }, timeout=30)
    response.raise_for_status()
    page = response.json()
    assert page['next_cursor'] is None
    return page['items']


health = requests.get(BASE + '/api/v1/health', timeout=10).json()
assert health['status'] == 'ok', health['status']
ensure = json.loads((RUN / 'site-ensure.json').read_text(encoding='utf-8'))
with ThreadPoolExecutor(max_workers=8) as pool:
    pages = list(pool.map(records, ensure['generation_roots']))
items = [item for page in pages for item in page]
strt = {item['refno']: item for item in items if item['generic'] == 'STRT'}
assert len(strt) == 82, len(strt)
missing = []
for item in items:
    for inst in item['insts']:
        if not (RUN / 'meshes' / (inst['geo_hash'] + '.mesh')).is_file():
            missing.append(inst['geo_hash'])
assert not missing, missing
branch = []
for word1, lo, hi in [(76730, -695, 525), (76731, 525, 1745), (76732, 1745, 2965)]:
    item = strt[f'24381_{word1}']
    bounds = item['world_aabb']
    assert abs(bounds['mins'][2] - lo) < 0.001
    assert abs(bounds['maxs'][2] - hi) < 0.001
    assert item['world_trans']['rotation'][:2] == [0.0, 0.0]
    assert (RUN / f'plant-ui-strt-{word1}.png').is_file()
    branch.append({'refno': item['refno'], 'z_mm': [lo, hi], 'height_mm': hi-lo})
binary = RUN.parent / 'aios-database.exe'
data = binary.read_bytes()
assert b'catalogue-route-scyl-paxi-frame-v1' in data
report = {
    'status': 'passed', 'backend': BASE, 'backend_sha256': hashlib.sha256(data).hexdigest(),
    'site': '24381/76564', 'loaded_records': len(items), 'strt_count': len(strt),
    'missing_mesh_files': 0, 'branch': branch,
    'screenshots': [f'plant-ui-strt-{word1}.png' for word1 in [76730, 76731, 76732]],
    'note': 'Screenshots are manually inspected real plant-ui captures; numeric checks read its backend API.',
}
(RUN / 'runtime-verification.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
(RUN / 'health.json').write_text(json.dumps(health, indent=2), encoding='utf-8')
print(json.dumps(report, indent=2))
