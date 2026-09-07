"""Catch missing FLEX middles even when the full element AABB still matches."""
from pathlib import Path
import json
import re
import sys
import argparse
import numpy as np
from cpv_site_audit import name_key, element_name
parser=argparse.ArgumentParser()
parser.add_argument('--audit-root',type=Path,default=Path(__file__).resolve().parents[1]/'output/hvac-sites-audit-20260905')
parser.add_argument('--require-complete',action='store_true')
args=parser.parse_args()
OUT=args.audit_root
results=[]
for folder in sorted(OUT.iterdir()):
    if not (folder/'generated/elements.json').exists(): continue
    native={name_key(g['name']):g for g in json.loads((folder/'native.json').read_text())}
    for e in json.loads((folder/'generated/elements.json').read_text()):
        if e['noun']!='FLEX': continue
        g=native[name_key(element_name(e))]
        world=np.eye(4);world[:3]=np.array(e['world']).T;inverse=np.linalg.inv(world)
        probes=[]
        for primitive in g['geometries']:
            if primitive['kind']!='Snout': continue
            params={k:float(v) for k,v in re.findall(r'(\w+): ([\d.eE+-]+)',primitive['params'])}
            if params['height']<=2 or min(params['radius_bottom'],params['radius_top'])<=0: continue
            t=np.eye(4);t[:3]=np.array(primitive['transform_cols_m']).reshape(4,3).T*1000
            t=inverse@t
            # These circular FLEX bodies run along local Z. Refuse other frames.
            assert abs(abs(t[2,2])-1)<1e-5
            z=float(t[2,3])
            covered=any(c['local_aabb_min'][2]-0.001<=z<=c['local_aabb_max'][2]+0.001 for c in e['components'])
            probes.append(dict(native_body_height_mm=params['height'],mid_z_mm=z,any_generated_component_at_midplane=covered))
        missing=any(not p['any_generated_component_at_midplane'] for p in probes)
        results.append(dict(refno=e['refno'],name=element_name(e),scope=folder.name,status='missing_middle' if missing else 'midplane_covered' if probes else 'not_circular_snout',
                            probes=probes,generated_z_intervals=[[c['local_aabb_min'][2],c['local_aabb_max'][2]] for c in e['components']]))
report=dict(flex_count=len(results),missing_middle=sum(r['status']=='missing_middle' for r in results),results=results)
(OUT/'flex-continuity.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
print(json.dumps({k:v for k,v in report.items() if k!='results'}))
for r in results:
    if r['status']=='missing_middle': print(r['refno'],r['name'],r['probes'][0]['native_body_height_mm'])
if '--require-complete' in sys.argv and report['missing_middle']: raise SystemExit(1)
