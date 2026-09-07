"""Assert native BEND data and model deltas through API and durable RocksDB."""
import json
import sys

import equi8000_live as e


phase = sys.argv[1]
folder = e.OUT / phase
moved = phase.endswith('moved')
restored = phase.endswith('restored-fixed')
expected = {
    'NAME': '/CODEX_TYPES_REMAIN_BEND_SREV',
    'POS': [-4291.87, -9373.71, 3000.0 if moved or restored else 2650.0],
    'ORI': [0.0, 0.0, 90.0],
    'ORIL': True,
    'SHOP': moved,
    'BUIL': moved,
    'SPRE': '13244/108845',
}

raw = e.api('element/attributes', {'refno': '24384/26656'})
api = {item['name']: item['value'] for item in raw['attributes']}
durable_rows = e.sql('SELECT * FROM BEND:24384_26656;')[0]
assert len(durable_rows) == 1
durable = durable_rows[0]

checks = []
for name, wanted in expected.items():
    got_api = api.get(name)
    got_durable = durable.get(name)
    if name == 'SPRE':
        got_durable = str(got_durable).removeprefix('pe:').replace('_', '/')
    if name in {'POS', 'ORI'}:
        api_ok = len(got_api) == len(wanted) and max(abs(a - b) for a, b in zip(got_api, wanted)) < 0.01
        durable_ok = len(got_durable) == len(wanted) and max(abs(a - b) for a, b in zip(got_durable, wanted)) < 0.01
    else:
        api_ok = got_api == wanted
        durable_ok = got_durable == wanted
    checks.append({
        'name': name,
        'expected': wanted,
        'api': got_api,
        'durable': got_durable,
        'passed': api_ok and durable_ok,
    })

result = {'passed': all(check['passed'] for check in checks), 'checks': checks}
e.save(folder / 'bend-attribute-comparison.json', result)
print(json.dumps(result, ensure_ascii=False))
assert result['passed']

if moved or restored:
    before = json.loads((e.OUT / 'remaining-bend-created-fixed' / 'records.json').read_text())
    after = json.loads((folder / 'records.json').read_text())
    def record(snapshot):
        return next(item for item in snapshot['items'] if item['refno'] == '24384_26656')
    a, b = record(before), record(after)
    mins_delta = [y - x for x, y in zip(a['world_aabb']['mins'], b['world_aabb']['mins'])]
    maxs_delta = [y - x for x, y in zip(a['world_aabb']['maxs'], b['world_aabb']['maxs'])]
    expected_delta = [0.0, 0.0, 350.0]
    delta_ok = max(abs(x - y) for x, y in zip(mins_delta, expected_delta)) < 0.01
    delta_ok &= max(abs(x - y) for x, y in zip(maxs_delta, expected_delta)) < 0.01
    delta = {
        'expected_world_delta': expected_delta,
        'mins_delta': mins_delta,
        'maxs_delta': maxs_delta,
        'same_geo_hash': a['insts'][0]['geo_hash'] == b['insts'][0]['geo_hash'],
        'passed': delta_ok and a['insts'][0]['geo_hash'] == b['insts'][0]['geo_hash'],
    }
    e.save(folder / 'bend-model-delta.json', delta)
    print(json.dumps(delta))
    assert delta['passed']
