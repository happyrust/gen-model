"""Summarize the final, saved equipment increment and restart evidence."""
import json
from equi8000_live import OUT,REPO,save
stages=['profiles-final-created','profiles-final-resized','profiles-final-moved','profiles-final-restarted']
summary=[]
for stage in stages:
    r=json.loads((OUT/stage/'types-comparison.json').read_text())
    summary.append(dict(stage=stage,passed=r['passed'],models=len(r['comparisons']),data_checks=len(r['data_checks']),rays=sum(len(c.get('ray_probes',[])) for c in r['comparisons']),max_volume_error=max(c['relative_volume_error'] for c in r['comparisons']),max_bounds_mm=max(c['bounds_error_mm'] for c in r['comparisons'])))
folder=OUT.parent/'increment23-persistence-quiescent'
a=json.loads((folder/'before-manifest.json').read_text());b=json.loads((folder/'after-manifest.json').read_text())
assert a==b
report=dict(stages=summary,persistence=dict(passed=True,tables=len(a),rows=sum(t['count'] for t in a.values())))
deleted=json.loads((OUT/'types-final-restored'/'deletion-softdelete-check.json').read_text())
assert deleted['passed']
report['deletion']={k:deleted[k] for k in ['passed','models','tree_nodes','checked_attribute_records']}
save(OUT/'final-23-summary.json',report)
print(json.dumps(report,indent=2))
