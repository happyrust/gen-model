"""Compare SCTN assemblies with their native owned FITT geometry included."""
from pathlib import Path
import copy
import json
import cpv_site_audit as audit

root = Path(__file__).resolve().parents[1]
baseline = root/'output/hvac-sites-audit-20260905'
out = root/'output/hvac-flex-gril-fix-20260905'
summaries = []
for target in json.loads((baseline/'targets.json').read_text(encoding='utf-8')):
    folder = out/target['refno'].replace('/', '_')
    elements = json.loads((folder/'generated/elements.json').read_text())
    by_ref = {e['refno']: e for e in elements}
    native = json.loads((folder/'native.json').read_text())
    by_name = {audit.name_key(g['name']): copy.deepcopy(g) for g in native}
    nodes = json.loads((baseline/folder.name/'tree-nodes.json').read_text())
    merged = []
    for node in nodes:
        if node['noun'] != 'FITT' or node['refno'].replace('_','/') in by_ref:
            continue
        parent = by_ref.get(node['owner'].replace('_','/'))
        if not parent or parent['noun'] != 'SCTN':
            continue
        child_key = audit.name_key(node['label'])
        parent_key = audit.name_key(audit.element_name(parent))
        child = by_name.get(child_key)
        host = by_name.get(parent_key)
        if child is None or host is None or len(parent['components']) < 2:
            continue
        host['geometries'].extend(child['geometries'])
        del by_name[child_key]
        merged.append(dict(host_refno=parent['refno'],fitting_refno=node['refno'].replace('_','/'),name=child_key))
    grouped_path = folder/'native-owned-fittings.json'
    grouped_path.write_text(json.dumps(list(by_name.values())))
    report = audit.main(folder/'generated/elements.json',grouped_path,folder/'owned-fittings-audit.json')
    verdict = {e['refno']:e['status'] for e in report['results'] if 'refno' in e}
    # A grouped mismatch remains visible; grouping is never itself a pass.
    for row in merged:
        row['host_bounds_status'] = verdict.get(row['host_refno'])
    (folder/'owned-fittings-mapping.json').write_text(json.dumps(merged,indent=2))
    summaries.append({**target,**{k:v for k,v in report.items() if k!='results'},
                      'owned_fittings_grouped':len(merged),
                      'grouped_hosts_failing':sum(r['host_bounds_status']!='bounds_pass' for r in merged)})
(out/'owned-fittings-summary.json').write_text(json.dumps(summaries,indent=2,ensure_ascii=False),encoding='utf-8')
