"""Assert SCTO/SBOX catalogue increments through API, RocksDB and model records."""
import json
import sys

import equi8000_live as e


CASES = {
    'scto': {
        'table': 'BEND',
        'ref': '24384/26661',
        'name': '/CODEX_TYPES_REMAIN_SCTO_BEND',
        'spre': '13244/109849',
        'native_pos': [-8728.0, -11487.0, 70.0],
        'delta': [275.0, 0.0, 0.0],
        'modelled': True,
    },
    'sbox': {
        'table': 'ATTA',
        'ref': '24384/26675',
        'name': '/CODEX_TYPES_REMAIN_SBOX_ATTA',
        'spre': '13244/108749',
        'native_pos': [-6989.0, 21916.0, 2200.0],
        'delta': [0.0, 225.0, 0.0],
        'modelled': False,
    },
    'broad': {
        'table': 'VALV',
        'ref': '24384/26685',
        'name': '/CODEX_TYPES_REMAIN_BROAD_VALV',
        'spre': '13245/851590',
        'native_pos': [-2685.0, -8479.0, 659.0],
        'delta': [0.0, 0.0, 325.0],
        'modelled': True,
        'before_phase': 'remaining-broad-moved-back',
        'after_phase': 'remaining-broad-moved-again',
    },
    'redu': {
        'table': 'REDU',
        'ref': '24384/26698',
        'name': '/CODEX_TYPES_REMAIN_REDU_TARGET',
        'spre': '13244/109009',
        'native_pos': [-8410.0, 13652.0, 3600.0],
        'delta': [0.0, 0.0, 300.0],
        'modelled': False,
    },
}

case_name = sys.argv[1]
case = CASES[case_name]
phase = case.get('after_phase', f'remaining-{case_name}-moved')
folder = e.OUT / phase
ref = case['ref']
record_ref = ref.replace('/', '_')

raw = e.api('element/attributes', {'refno': ref})
api = {item['name']: item['value'] for item in raw['attributes']}
durable_rows = e.sql(f"SELECT * FROM {case['table']}:{record_ref};")[0]
assert len(durable_rows) == 1
durable = durable_rows[0]

expected = {
    'NAME': case['name'],
    'SPRE': case['spre'],
    'SHOP': True,
    'BUIL': True,
}
checks = []
for name, wanted in expected.items():
    got_api = api.get(name)
    got_durable = durable.get(name)
    if name == 'SPRE':
        got_durable = str(got_durable).removeprefix('pe:').replace('_', '/')
    checks.append({
        'name': name,
        'expected': wanted,
        'api': got_api,
        'durable': got_durable,
        'passed': got_api == wanted and got_durable == wanted,
    })

for name in ('POS', 'ORI', 'ORIL'):
    got_api = api.get(name)
    got_durable = durable.get(name)
    if name == 'POS':
        expected_pos = case['native_pos']
        native_ok = max(abs(a - b) for a, b in zip(got_api, expected_pos)) < 0.6
    else:
        native_ok = True
    if isinstance(got_api, list):
        equal = len(got_api) == len(got_durable) and max(
            abs(a - b) for a, b in zip(got_api, got_durable)
        ) < 0.01
    else:
        equal = got_api == got_durable
    checks.append({
        'name': name,
        'api': got_api,
        'durable': got_durable,
        'passed': equal and native_ok,
    })

before_phase = case.get('before_phase', f'remaining-{case_name}-created')
before = json.loads((e.OUT / before_phase / 'records.json').read_text())['items']
after = json.loads((folder / 'records.json').read_text())['items']
target_before = [item for item in before if item['refno'] == record_ref]
target_after = [item for item in after if item['refno'] == record_ref]
if case['modelled']:
    assert len(target_before) == len(target_after) == 1
    a, b = target_before[0], target_after[0]
    mins_delta = [y - x for x, y in zip(a['world_aabb']['mins'], b['world_aabb']['mins'])]
    maxs_delta = [y - x for x, y in zip(a['world_aabb']['maxs'], b['world_aabb']['maxs'])]
    delta_ok = max(abs(x - y) for x, y in zip(mins_delta, case['delta'])) < 0.01
    delta_ok &= max(abs(x - y) for x, y in zip(maxs_delta, case['delta'])) < 0.01
    model = {
        'modelled': True,
        'mins_delta': mins_delta,
        'maxs_delta': maxs_delta,
        'expected_delta': case['delta'],
        'same_geo_hash': a['insts'][0]['geo_hash'] == b['insts'][0]['geo_hash'],
        'passed': delta_ok and a['insts'][0]['geo_hash'] == b['insts'][0]['geo_hash'],
    }
else:
    # AMS8000's native RVM exporter also omits these members. They remain
    # data-only even though their SPRE references a catalogue component.
    model = {
        'modelled': False,
        'target_records_before': len(target_before),
        'target_records_after': len(target_after),
        'native_rvm_target_absent': True,
        'passed': not target_before and not target_after,
    }

result = {
    'passed': all(check['passed'] for check in checks) and model['passed'],
    'checks': checks,
    'model': model,
}
e.save(folder / f'{case_name}-attribute-model-comparison.json', result)
print(json.dumps(result, ensure_ascii=False))
assert result['passed']
