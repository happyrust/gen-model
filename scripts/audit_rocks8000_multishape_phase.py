"""Compare polygon phase of the 25.05 mm horizontal cylinder in RVM/runtime."""
import json, sys
import numpy as np
import equi8000_live as e

folder=e.OUT/sys.argv[1]
group=next(g for g in json.loads((folder/'rvm.json').read_text()) if g['name']=='/CODEX_TYPES_MULTI_VALV')
geo=group['geometries'][1]
points=np.array([p for poly in geo['polygons_world_mm'] for ring in poly for p in ring])
def angles(points):
    selected=points[np.abs(np.hypot(points[:,1]-1400,points[:,2])-25.05)<.02]
    values=sorted({round(float(np.degrees(np.arctan2(p[2],p[1]-1400)))%360,3) for p in selected})
    return values
records=json.loads((folder/'records.json').read_text())['items']
record_index,record=next((i,r) for i,r in enumerate(records) if r['generic']=='VALV')
mesh=json.loads((folder/f'route-{record_index}-0-mesh.json').read_text())
verts=np.array(mesh['vertices'])+np.array(record['world_trans']['translation'])
# The target cylinder spans world X=5000..5080 and has radius 25.05 in YZ.
candidate=verts[(np.abs(np.hypot(verts[:,1]-1400,verts[:,2])-25.05)<.02) &
                ((np.abs(verts[:,0]-5000)<.02)|(np.abs(verts[:,0]-5080)<.02))]
result={'rvm_angles_deg':angles(points),'runtime_angles_deg':angles(candidate)}
result['rvm_segments']=len(result['rvm_angles_deg']);result['runtime_segments']=len(result['runtime_angles_deg'])
result['same_phase']=result['rvm_angles_deg']==result['runtime_angles_deg']
e.save(folder/'cylinder-phase-audit.json',result)
print(result)
