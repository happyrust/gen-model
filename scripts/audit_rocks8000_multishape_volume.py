"""Diagnose component winding/overlap without changing the acceptance test."""
import json, sys
import numpy as np
import equi8000_live as e

folder=e.OUT/sys.argv[1]
group=next(g for g in json.loads((folder/'rvm.json').read_text()) if g['name']=='/CODEX_TYPES_MULTI_VALV')
parts=[]
for index,geo in enumerate(group['geometries']):
    rings=[np.array(r) for p in geo['polygons_world_mm'] for r in p]
    center=np.concatenate(rings).mean(0)
    signed=sum(float(np.dot(r[0]-center,np.cross(r[i]-center,r[i+1]-center)))/6
               for r in rings for i in range(1,len(r)-1))
    parts.append({'index':index,'kind':geo['kind'],'signed_volume':signed,'params':geo['params']})
mesh=json.loads((folder/'route-0-0-mesh.json').read_text())
verts=np.array(mesh['vertices']); center=verts.mean(0)
volume=abs(sum(float(np.dot(verts[a]-center,np.cross(verts[b]-center,verts[c]-center)))/6
               for a,b,c in np.array(mesh['indices']).reshape(-1,3)))
duplicates=[]
for i,geo in enumerate(group['geometries']):
    for j,earlier in enumerate(group['geometries'][:i]):
        if geo['params']==earlier['params'] and geo['transform_cols_m']==earlier['transform_cols_m']:
            duplicates.append([j,i])
result={'parts':parts,'duplicate_rvm_placements':duplicates,'rvm_signed_sum':sum(p['signed_volume'] for p in parts),
        'rvm_absolute_component_sum':sum(abs(p['signed_volume']) for p in parts),
        'local_mesh_volume':volume,'note':'Diagnostic only; local mesh volume requires instance scale before comparing. Component sums double-count overlap.'}
e.save(folder/'volume-audit.json',result)
print(json.dumps(result))
