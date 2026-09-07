"""Compare actual PlantMesh and RVM surface volume and through-hole probes."""
import json,sys,subprocess
import numpy as np
from equi8000_live import OUT,REPO,save

phase=sys.argv[1]; folder=OUT/phase
row=next(r for r in json.loads((folder/'records.json').read_text())['items'] if r['refno']=='24384_26292')
inst=row['insts'][0]
mesh=REPO/'output/rocksdb-8000-20260905/meshes'/ (inst['geo_hash']+'.mesh')
subprocess.run(['D:/Rust/target/debug/examples/pipe_mesh_dump.exe',str(mesh),str(folder/'plant-mesh.json')],check=True)
def matrix(t):
    x,y,z,w=t['rotation']
    r=np.array([[1-2*(y*y+z*z),2*(x*y-z*w),2*(x*z+y*w)],
                [2*(x*y+z*w),1-2*(x*x+z*z),2*(y*z-x*w)],
                [2*(x*z-y*w),2*(y*z+x*w),1-2*(x*x+y*y)]])
    m=np.eye(4);m[:3,:3]=r@np.diag(t['scale']);m[:3,3]=t['translation'];return m
world=matrix(row['world_trans']); local=matrix(inst['transform'])
def apply(points,m):return (np.c_[points,np.ones(len(points))]@m.T)[:,:3]
data=json.loads((folder/'plant-mesh.json').read_text())
vertices=apply(np.array(data['vertices']),world@local)
indices=np.array(data['indices']).reshape(-1,3)
plant=[[vertices[t]] for t in indices]
groups=json.loads((folder/'rvm.json').read_text())
rvm=[[np.array(ring) for ring in poly] for g in groups if g['name']=='/CODEX_ROCKS8000_BOOL_BASE' for geo in g['geometries'] for poly in geo['polygons_world_mm']]
# Work in the BOX element frame: less cancellation and meaningful hole coordinates.
inv=np.linalg.inv(world)
plant=[[apply(r,inv) for r in p] for p in plant]
rvm=[[apply(r,inv) for r in p] for p in rvm]
def volume(polys):
    return abs(sum(float(np.dot(r[0],np.cross(r[i],r[i+1])))/6 for p in polys for r in p for i in range(1,len(r)-1)))
def inside(point,ring):
    x,y=point;result=False
    for a,b in zip(ring,np.roll(ring,-1,axis=0)):
        if (a[1]>y)!=(b[1]>y) and x<(b[0]-a[0])*(y-a[1])/(b[1]-a[1])+a[0]:result=not result
    return result
def hits(polys,x,y):
    zs=[]
    for p in polys:
        outer=p[0]
        normal=next((n for i in range(1,len(outer)-1) if np.linalg.norm(n:=np.cross(outer[i]-outer[0],outer[i+1]-outer[0]))>1e-7),None)
        if normal is None or abs(normal[2])<1e-6:continue
        if not inside([x,y],outer[:,:2]) or any(inside([x,y],r[:,:2]) for r in p[1:]):continue
        z=outer[0,2]-(normal[0]*(x-outer[0,0])+normal[1]*(y-outer[0,1]))/normal[2]
        if not any(abs(z-a)<.01 for a in zs):zs.append(float(z))
    return len(zs)
width=float(sys.argv[2]);cx=float(sys.argv[3]) if len(sys.argv)>3 else 0
expected=(1000*1000-width*300)*400
pv,rv=volume(plant),volume(rvm)
probes=[]
for x,y in [(cx+13,17),(cx-width/2-20,17),(cx+width/2+20,17),(413,413)]:
    expected_hits=0 if width and abs(x-cx)<width/2 and abs(y)<150 else 2
    probes.append(dict(x=x,y=y,expected=expected_hits,plant=hits(plant,x,y),rvm=hits(rvm,x,y)))
result=dict(mesh=inst['geo_hash'],plant_volume_mm3=pv,rvm_volume_mm3=rv,analytic_volume_mm3=expected,
            relative_volume_error=abs(pv-rv)/expected,probes=probes,
            passed=abs(pv-rv)/expected<1e-5 and abs(pv-expected)/expected<1e-5 and all(p['plant']==p['rvm']==p['expected'] for p in probes))
save(folder/'boolean-comparison.json',result);print(json.dumps(result));assert result['passed']
