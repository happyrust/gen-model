"""Match each native FITT box to a separate generated component, not the host AABB."""
from pathlib import Path
import itertools
import json
import re
import numpy as np
import cpv_site_audit as audit

out = Path(__file__).resolve().parents[1]/'output/hvac-flex-gril-fix-20260905'
results = []
for mapping_path in list(out.glob('243*/owned-fittings-mapping.json')) + list(out.glob('cpv-regression/owned-fittings-mapping.json')):
    folder = mapping_path.parent
    native = {audit.name_key(g['name']):g for g in json.loads((folder/'native.json').read_text())}
    elements = {e['refno']:e for e in json.loads((folder/'generated/elements.json').read_text())}
    used = set()
    for mapping in json.loads(mapping_path.read_text()):
        host = elements[mapping['host_refno']]
        geometries = native[mapping['name']]['geometries']
        assert len(geometries) == 1 and geometries[0]['kind'] == 'Box'
        box = geometries[0]
        lengths = np.array(json.loads(re.search(r'lengths: (\[[^]]+\])', box['params'])[1]))
        native_transform = np.eye(4)
        native_transform[:3] = np.array(box['transform_cols_m']).reshape(4,3).T*1000
        host_transform = np.eye(4)
        host_transform[:3] = np.array(host['world']).T
        transform = np.linalg.inv(host_transform) @ native_transform
        points = np.array(list(itertools.product([-1,1],repeat=3))) * lengths/2
        points = points @ transform[:3,:3].T + transform[:3,3]
        expected = np.r_[points.min(axis=0),points.max(axis=0)]
        candidates = []
        for index,component in enumerate(host['components']):
            if (host['refno'],index) in used:
                continue
            bounds = np.array(component['local_aabb_min']+component['local_aabb_max'])
            candidates.append((float(abs(bounds-expected).max()),index,component))
        error,index,component = min(candidates,key=lambda c:c[0])
        used.add((host['refno'],index))
        relative_volume_error = abs(component['volume_mm3']/float(np.prod(lengths))-1)
        results.append(dict(**mapping,component=index,error_mm=error,
            relative_volume_error=relative_volume_error,
            passed=error<=1.1 and relative_volume_error<1e-5))
report = dict(count=len(results),failed=sum(not r['passed'] for r in results),
    max_error_mm=max(r['error_mm'] for r in results),
    max_relative_volume_error=max(r['relative_volume_error'] for r in results),results=results)
(out/'fitting-parts-audit.json').write_text(json.dumps(report,indent=2))
print(json.dumps({k:v for k,v in report.items() if k!='results'}))
assert report['failed'] == 0
