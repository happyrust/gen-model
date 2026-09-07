"""Collect the confirmed FLEX/GRIL defects, including AABB false negatives."""
import csv
import json
from pathlib import Path

out = Path(__file__).resolve().parents[1] / 'output/hvac-sites-audit-20260905'
rows = []
for result in json.loads((out / 'flex-continuity.json').read_text())['results']:
    if result['status'] == 'missing_middle':
        rows.append(dict(refno=result['refno'], noun='FLEX', name=result['name'],
                         defect='missing_middle',
                         dimension_mm=result['probes'][0]['native_body_height_mm']))
for target in json.loads((out / 'targets.json').read_text(encoding='utf-8')):
    folder = out / target['refno'].replace('/', '_')
    for result in json.loads((folder / 'audit.json').read_text())['results']:
        if result['status'] == 'bounds_fail' and result.get('noun') == 'GRIL':
            rows.append(dict(refno=result['refno'], noun='GRIL', name=result['name'],
                             defect='extra_box', dimension_mm=result['error_mm']))
with (out / 'vent-defects.csv').open('w', newline='', encoding='utf-8-sig') as stream:
    writer = csv.DictWriter(stream, fieldnames=['refno', 'noun', 'name', 'defect', 'dimension_mm'])
    writer.writeheader()
    writer.writerows(rows)
print(f'{len(rows)} confirmed FLEX/GRIL defects')
