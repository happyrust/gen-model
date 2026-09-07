"""Verify the real published move independently of absolute shape accuracy."""
import json
import numpy as np
import equi8000_live as e

folders=[e.OUT/('multishape-valve-'+phase) for phase in ('created','moved')]
records=[]; bounds=[]
for folder in folders:
    assert not (folder/'publication-timeout.json').exists()
    data=json.loads((folder/'records.json').read_text())
    records.append(next(r for r in data['items'] if r['refno']=='24384_26653'))
    group=next(g for g in json.loads((folder/'rvm.json').read_text()) if g['name']=='/CODEX_TYPES_MULTI_VALV')
    points=np.array([p for g in group['geometries'] for poly in g['polygons_world_mm'] for ring in poly for p in ring])
    bounds.append(np.r_[points.min(0),points.max(0)])
before,after=records
meshes=[json.loads((folder/'route-0-0-mesh.json').read_text()) for folder in folders]
same_topology=meshes[0]['indices']==meshes[1]['indices']
v0,v1=[np.array(m['vertices']) for m in meshes]
vertex_error=float(np.max(np.abs(v1-v0))) if v0.shape==v1.shape else None
delta=(np.array(after['world_trans']['translation'])-np.array(before['world_trans']['translation'])).tolist()
rvm_delta=(bounds[1]-bounds[0]).tolist()
checks={'translation_is_700_north':np.allclose(delta,[0,700,0],atol=.001,rtol=0),
        'rvm_translation_is_700_north':np.allclose(rvm_delta,[0,700,0,0,700,0],atol=.001,rtol=0),
        'local_mesh_topology_unchanged':same_topology,
        'local_vertex_error_below_001mm':vertex_error is not None and vertex_error<.001,
        'local_instance_transforms_unchanged':[i['transform'] for i in before['insts']]==[i['transform'] for i in after['insts']],
        'orientation_scale_unchanged':all(before['world_trans'][k]==after['world_trans'][k] for k in ('rotation','scale'))}
result={'passed':bool(all(checks.values())),'checks':{k:bool(v) for k,v in checks.items()},
        'translation_delta':delta,'rvm_bounds_delta':rvm_delta,'local_vertex_error_mm':vertex_error,
        'hash_unchanged':before['insts']==after['insts'],
        'scope':'Automatic position increment only; does not close the absolute geometry discrepancy.'}
e.save(folders[1]/'model-delta-comparison.json',result)
print(result)
assert result['passed']
