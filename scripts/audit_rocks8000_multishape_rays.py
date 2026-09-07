"""Compare occupied ray intervals of RVM component union and rendered mesh.

Diagnostic coverage, not a replacement for the failing volume acceptance check.
"""
import json, sys
import numpy as np
import equi8000_live as e

folder=e.OUT/sys.argv[1]
group_name=sys.argv[2] if len(sys.argv)>2 else '/CODEX_TYPES_MULTI_VALV'
record_ref=sys.argv[3].replace('/','_') if len(sys.argv)>3 else None
group=next(g for g in json.loads((folder/'rvm.json').read_text()) if g['name']==group_name)
records=json.loads((folder/'records.json').read_text())['items']
record_index,record=next(
    (i,r) for i,r in enumerate(records)
    if r['generic']=='VALV' and (record_ref is None or r['refno']==record_ref)
)
mesh=json.loads((folder/f'route-{record_index}-0-mesh.json').read_text())

def transform(v,t):
    q=np.array(t['rotation']); a=v*np.array(t['scale'])
    return a+2*(q[3]*np.cross(q[:3],a)+np.cross(q[:3],np.cross(q[:3],a)))+np.array(t['translation'])

vertices=transform(transform(np.array(mesh['vertices']),record['insts'][0]['transform']),record['world_trans'])
plant=vertices[np.array(mesh['indices']).reshape(-1,3)]
tri_indices=np.array(mesh['indices']).reshape(-1,3)
parts=[]
for geo in group['geometries']:
    assert all(len(p)==1 for p in geo['polygons_world_mm']), 'This convex primitive audit cannot triangulate holes'
    parts.append(np.array([[r[0],r[i],r[i+1]] for p in geo['polygons_world_mm'] for r in p for i in range(1,len(r)-1)]))

def intervals(tris,origin,axis):
    direction=np.eye(3)[axis]
    edge1=tris[:,1]-tris[:,0]; edge2=tris[:,2]-tris[:,0]
    h=np.cross(direction,edge2); det=np.sum(edge1*h,axis=1)
    valid=np.abs(det)>1e-9
    inv=np.divide(1.,det,out=np.zeros_like(det),where=valid)
    s=origin-tris[:,0]; u=np.sum(s*h,axis=1)*inv
    q=np.cross(s,edge1); v=np.sum(q*direction,axis=1)*inv
    ts=np.sum(edge2*q,axis=1)*inv
    values=sorted(ts[valid & (u>=-1e-8) & (v>=-1e-8) & (u+v<=1+1e-8)])
    unique=[]
    for value in values:
        if not unique or abs(value-unique[-1])>1e-4: unique.append(float(value))
    # A ray tangent to one independently tessellated part can report one
    # vertex hit after duplicate-face intersections are merged; it occupies no
    # interval and must not make the whole assembly look open.
    if len(unique)==1:
        return []
    assert len(unique)%2==0, ('Open ray',axis,origin.tolist(),unique)
    return list(zip(unique[::2],unique[1::2]))

def union(values):
    result=[]
    for a,b in sorted(values):
        if result and a<=result[-1][1]+1e-4: result[-1][1]=max(result[-1][1],b)
        else:result.append([a,b])
    return result

def winding_intervals(tris,origin,axis):
    """Occupied intervals from signed crossings; overlap keeps |winding| > 1."""
    direction=np.eye(3)[axis]
    edge1=tris[:,1]-tris[:,0]; edge2=tris[:,2]-tris[:,0]
    normals=np.cross(edge1,edge2); h=np.cross(direction,edge2)
    det=np.sum(edge1*h,axis=1); valid=np.abs(det)>1e-9
    inv=np.divide(1.,det,out=np.zeros_like(det),where=valid)
    s=origin-tris[:,0]; u=np.sum(s*h,axis=1)*inv
    q=np.cross(s,edge1); v=np.sum(q*direction,axis=1)*inv
    ts=np.sum(edge2*q,axis=1)*inv
    hit=valid & (u>=-1e-8) & (v>=-1e-8) & (u+v<=1+1e-8)
    events=sorted((float(t),1 if dot>0 else -1) for t,dot in zip(ts[hit],normals[hit]@direction))
    merged=[]
    for t,sign in events:
        if merged and abs(t-merged[-1][0])<1e-4: merged[-1][1]+=sign
        else: merged.append([t,sign])
    result=[]; winding=0; start=None
    for t,delta in merged:
        before=winding; winding+=delta
        if before==0 and winding!=0:start=t
        elif before!=0 and winding==0:result.append([start,t]);start=None
    assert winding==0 and start is None, ('Open oriented mesh',axis,origin.tolist(),merged)
    return result

bounds=np.concatenate(parts).reshape(-1,3)
lo=bounds.min(0); hi=bounds.max(0); rng=np.random.default_rng(8000)
checks=[]
for axis in range(3):
    for sample in range(200):
        origin=lo+(hi-lo)*rng.uniform(.001,.999,3); origin[axis]=lo[axis]-10
        part_intervals=[intervals(part,origin,axis) for part in parts]
        expected=union([ab for values in part_intervals for ab in values])
        actual=winding_intervals(plant,origin,axis)
        passed=len(actual)==len(expected) and all(abs(a-c)<2 and abs(b-d)<2 for (a,b),(c,d) in zip(actual,expected))
        check={'axis':axis,'origin':origin.tolist(),'expected':expected,'actual':actual,'passed':passed}
        if not passed:
            check['contributing_rvm_parts']=[{'index':i,'params':group['geometries'][i]['params'],
                                             'transform':group['geometries'][i]['transform_cols_m'],'intervals':values}
                                            for i,values in enumerate(part_intervals) if values]
        checks.append(check)
result={'passed':all(c['passed'] for c in checks),'rays':len(checks),'failures':sum(not c['passed'] for c in checks),
        'rvm_parts':len(parts),'plant_interval_method':'signed winding over concatenated components',
        'scope':'600 deterministic occupied intervals, 2 mm endpoint tolerance; finite sample, not full topology proof', 'checks':checks}
e.save(folder/'union-ray-audit.json',result)
print({k:v for k,v in result.items() if k!='checks'})
