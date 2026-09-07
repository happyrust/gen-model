"""Recheck the original CPV SITE after generic catalogue and FITT changes."""
from pathlib import Path
import json
import shutil
import cpv_site_audit as audit
import hvac_sites_audit

root = Path(__file__).resolve().parents[1]
out = root/'output/hvac-flex-gril-fix-20260905/cpv-regression'
out.mkdir(exist_ok=True)
hvac_sites_audit.generate({'refno':'24381/76564'},out)
shutil.copyfile(root/'output/strt-cpv-20260905/site-all-rvm.json',out/'native.json')
native = {audit.name_key(g['name']):g for g in json.loads((out/'native.json').read_text())}
elements = {e['refno']:e for e in json.loads((out/'generated/elements.json').read_text())}
mapping = []
for node in json.loads((root/'output/cpv-site-audit-20260905/fitt-tree-nodes.json').read_text()):
    host = elements[node['owner'].replace('_','/')]
    name = node['name'] if node['name'].startswith('/') else f"FITT {node['order']+1} of {audit.element_name(host)}"
    key = audit.name_key(name)
    native[audit.name_key(audit.element_name(host))]['geometries'].extend(native.pop(key)['geometries'])
    mapping.append(dict(host_refno=host['refno'],fitting_refno=node['refno'].replace('_','/'),name=key))
(out/'native-owned-fittings.json').write_text(json.dumps(list(native.values())))
report = audit.main(out/'generated/elements.json',out/'native-owned-fittings.json',out/'owned-fittings-audit.json')
verdict = {r['refno']:r['status'] for r in report['results'] if 'refno' in r}
for row in mapping:
    row['host_bounds_status'] = verdict[row['host_refno']]
(out/'owned-fittings-mapping.json').write_text(json.dumps(mapping,indent=2))
assert report['status'] == 'bounds_complete'
