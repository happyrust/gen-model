"""Inspect the saved live RVM failures without querying mutable database state."""
import json,sys
import numpy as np
from equi8000_live import OUT

folder = OUT / (sys.argv[1] if len(sys.argv)>1 else 'profiles-moved-result')
report = json.loads((folder / 'types-comparison.json').read_text())
groups = json.loads((folder / 'rvm.json').read_text())
for row in report['comparisons']:
    if row['passed']:
        continue
    group = next(g for g in groups if g['name'] == row['name'])
    print(row['name'], 'volume', row['plant_volume'], row['rvm_volume'])
    for geo in group['geometries']:
        print(' RVM', geo['kind'], geo['params'], 'polygons', len(geo['polygons_world_mm']))
        transform = np.eye(4)
        transform[:3, :] = np.array(geo['transform_cols_m']).reshape(4,3).T * 1000
        pts = np.concatenate([ring for poly in geo['polygons_world_mm'] for ring in poly])
        local = (np.c_[pts,np.ones(len(pts))] @ np.linalg.inv(transform).T)[:,:3]
        if row['name']=='/CODEX_TYPES_POHE':
            print(' bounds',local.min(0),local.max(0))
        if 'HOST_NCON' in row['name']:
            circle = local[np.abs(local[:,2]+300)<.01] - [120,0,0]
            print(' bottom circle angles', np.unique(np.round(np.degrees(np.arctan2(circle[:,1],circle[:,0])),2)))
        if 'REVO' in row['name'] or 'HOST_NREV' in row['name']:
            circle = local[np.abs(np.linalg.norm(local[:,1:3],axis=1)-500)<.01]
            print(' sweep angles', np.unique(np.round(np.degrees(np.arctan2(circle[:,2],circle[:,1])),2)))
        if 'HOST_NCTO' in row['name']:
            moved = 120 if folder.name=='profiles-moved-result' else 0
            center = 385 if folder.name=='profiles-moved-result' else 350
            p = local[np.abs(local[:,2])<399] - [moved,0,0]
            print(' torus ring angles', np.unique(np.round(np.degrees(np.arctan2(p[:,1],p[:,0])),2)))
            print(' torus tube angles', np.unique(np.round(np.degrees(np.arctan2(p[:,2],np.linalg.norm(p[:,:2],axis=1)-center)),2)))
    print(' failed rays', [r for r in row.get('ray_probes', []) if not r['passed']][:5])
