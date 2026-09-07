"""Explain cylinder mesh-volume differences without changing pass tolerances."""
import json, math, re
from pathlib import Path
import numpy as np

folder = Path(__file__).resolve().parents[1] / 'output/rocksdb-8000-20260905/equi-increment/catalogue-positive-clone'
geo = next(geo for g in json.loads((folder/'rvm.json').read_text())
           for geo in g['geometries'] if geo['kind'] == 'Cylinder')
radius, height = map(float, re.search(r'radius: ([\d.]+), height: ([\d.]+)', geo['params']).groups())
cols = np.array(geo['transform_cols_m'])
transform = cols[:9].reshape(3, 3).T * 1000
translation = cols[9:] * 1000
mesh = json.loads((folder/'route-3-0-mesh.json').read_text())
records = json.loads((folder/'records.json').read_text())['items']
record = records[3]
assert record['generic'] == 'TUBI'
def matrix(t):
    x,y,z,w=t['rotation'];m=np.eye(4)
    m[:3,:3]=np.array([[1-2*(y*y+z*z),2*(x*y-z*w),2*(x*z+y*w)],
                       [2*(x*y+z*w),1-2*(x*x+z*z),2*(y*z-x*w)],
                       [2*(x*z-y*w),2*(y*z+x*w),1-2*(x*x+y*y)]])@np.diag(t['scale'])
    m[:3,3]=t['translation'];return m
world=(np.c_[mesh['vertices'],np.ones(len(mesh['vertices']))] @
       (matrix(record['world_trans'])@matrix(record['insts'][0]['transform'])).T)[:,:3]
def inspect(points):
    local=(points-translation)@np.linalg.inv(transform).T
    radial=np.linalg.norm(local[:,:2],axis=1)
    rim=local[radial>radius*.5]
    raw_angles=np.mod(np.arctan2(rim[:,1],rim[:,0]),2*math.pi)
    raw_angles[np.minimum(raw_angles,2*math.pi-raw_angles)<.001]=0
    angles=np.unique(np.round(raw_angles,2))
    n=len(angles)
    return {'circumferential_segments':n,
            'rim_radius_max_error_mm':float(abs(np.linalg.norm(rim[:,:2],axis=1)-radius).max()),
            'end_plane_max_error_mm':float(abs(abs(local[:,2])-height/2).max()),
            'regular_polygon_volume_ratio':n*math.sin(2*math.pi/n)/(2*math.pi),
            'regular_polygon_sagitta_mm':radius*(1-math.cos(math.pi/n))}
rvm=np.array(geo['polygons_world_mm']).reshape(-1,3)
result={'analytic_radius_mm':radius,'analytic_height_mm':height,
        'runtime':inspect(world),'rvm_export_tessellation':inspect(rvm),
        'scope':'Diagnostic only; does not supersede route-comparison failure or prove topology.'}
result['predicted_mesh_volume_relative_difference']=abs(result['runtime']['regular_polygon_volume_ratio']/result['rvm_export_tessellation']['regular_polygon_volume_ratio']-1)
(folder/'cylinder-tessellation-audit.json').write_text(json.dumps(result,indent=2))
print(json.dumps(result,indent=2))
