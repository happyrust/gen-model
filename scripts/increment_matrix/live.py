"""Observe persisted live records; never request regeneration to make a gate pass."""
import math
import json
import time
import urllib.request
from pathlib import Path


def converged(row, pending, target):
    return (row.get('applied_sesno') == target and row.get('model_sesno') == target
            and row.get('model_verdict') == 'in_sync'
            and pending.get('units') == [] and pending.get('dead_letters') == 0)


def api(url, route, body=None):
    request = urllib.request.Request(url.rstrip('/') + '/api/v1/' + route,
        data=None if body is None else json.dumps(body).encode(),
        headers={'Content-Type': 'application/json'})
    with urllib.request.urlopen(request, timeout=15) as response:
        return json.load(response)


def observe(url, case, out, timeout=180):
    out = Path(out)
    out.mkdir(exist_ok=False)
    deadline = time.monotonic() + timeout
    observations = []
    result = {'passed': False, 'endpoint': url, 'target': case['target'], 'dbnum': case['dbnum'],
              'observations': observations}
    roots = sorted(set(case['before_roots'] + case['after_roots']))
    while time.monotonic() < deadline:
        try:
            rows = api(url, 'dbnums')['dbnums']
            row = next(r for r in rows if r['dbnum'] == case['dbnum'])
            pending = api(url, f"update/pending-units?dbnum={case['dbnum']}")
            (out / 'pending.json').write_text(json.dumps(pending, indent=2))
            observed = {'row': row, 'pending': {
                'units_count': len(pending['units']),
                'dead_letters': pending['dead_letters'],
                'target_units': [unit for unit in pending['units'] if unit.get('root_refno') in roots]}}
            if not observations or observed != observations[-1]:
                observations.append(observed)
            comparisons = {}
            for root in roots:
                records, cursor = [], 0
                while True:
                    page = api(url, 'model/records', {'generation_root': root, 'cursor': cursor})
                    if page.get('source') != 'model-database':
                        raise ValueError('durable live test requires model-database records')
                    records += page['items']
                    if not page['truncated']:
                        break
                    next_cursor = page['next_cursor']
                    if not isinstance(next_cursor, int) or next_cursor <= cursor:
                        raise ValueError('records pagination did not advance')
                    cursor = next_cursor
                label = root.replace('/', '-')
                expected = json.loads((out.parent / f'after-{label}/elements.json').read_text()) if root in case['after_roots'] else []
                (out / f'{label}-records.json').write_text(json.dumps(records, indent=2))
                comparisons[root] = compare_records(expected, records)
            result['comparisons'] = comparisons
            result['passed'] = converged(row, pending, case['target']) and all(c['passed'] for c in comparisons.values())
            if result['passed']:
                break
        except (OSError, ValueError, KeyError, StopIteration) as error:
            result['last_error'] = str(error)
        (out / 'acceptance.json').write_text(json.dumps(result, indent=2))
        time.sleep(2)
    (out / 'acceptance.json').write_text(json.dumps(result, indent=2))
    return result


def compare_records(expected, actual):
    remaining = list(actual)
    errors = []
    for element in expected:
        bounds = element['aabb_min'] + element['aabb_max']
        refno = element['refno'].replace('/', '_')
        candidates = [i for i, row in enumerate(remaining)
                      if row['refno'].replace('/', '_') == refno
                      and bool(row.get('insts'))
                      and any(inst.get('is_tubi', False) for inst in row['insts'])
                      == (element['geometry_id']['kind'] != 'element')]
        match = None
        for i in candidates:
            box = remaining[i]['world_aabb']
            observed = box['mins'] + box['maxs']
            if len(observed) == 6 and all(math.isfinite(a) and math.isfinite(b)
                    and abs(a - b) <= .01 for a, b in zip(bounds, observed)):
                match = i
                break
        if match is None:
            errors.append(f'missing or wrong bounds: {refno}')
        else:
            remaining.pop(match)
    if remaining:
        errors.append(f'{len(remaining)} unexpected records')
    return {"passed": not errors, "expected": len(expected), "actual": len(actual),
            "errors": errors, "tolerance_mm": .01, "scope": "membership and world AABB, not topology"}
