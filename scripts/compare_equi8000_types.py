"""Save actual mesh/RVM comparisons and direct API versus durable attributes."""
import json,sys,subprocess
import numpy as np
import equi8000_live as e
from equi8000_types import P,specs

def matrix(t):
    x,y,z,w=t['rotation'];m=np.eye(4)
    m[:3,:3]=np.array([[1-2*(y*y+z*z),2*(x*y-z*w),2*(x*z+y*w)],[2*(x*y+z*w),1-2*(x*x+z*z),2*(y*z-x*w)],[2*(x*z-y*w),2*(y*z+x*w),1-2*(x*x+y*y)]])@np.diag(t['scale'])
    m[:3,3]=t['translation'];return m
def apply(p,m):return (np.c_[p,np.ones(len(p))]@m.T)[:,:3]
def volume(polys):
    return abs(sum(float(np.dot(r[0],np.cross(r[i],r[i+1])))/6 for p in polys for r in p for i in range(1,len(r)-1)))
def inside(point,ring):
    x,y=point;hit=False
    for a,b in zip(ring,np.roll(ring,-1,axis=0)):
        if (a[1]>y)!=(b[1]>y) and x<(b[0]-a[0])*(y-a[1])/(b[1]-a[1])+a[0]:hit=not hit
    return hit
def ray_hits(polys,x,y):
    zs=[]
    for poly in polys:
        ring=poly[0]
        normal=next((n for i in range(1,len(ring)-1) if np.linalg.norm(n:=np.cross(ring[i]-ring[0],ring[i+1]-ring[0]))>1e-7),None)
        if normal is None or abs(normal[2])<1e-6:continue
        if not inside([x,y],ring[:,:2]) or any(inside([x,y],r[:,:2]) for r in poly[1:]):continue
        z=ring[0,2]-(normal[0]*(x-ring[0,0])+normal[1]*(y-ring[0,1]))/normal[2]
        if not any(abs(z-a)<.01 for a in zs):zs.append(float(z))
    return sorted(zs)
folder=e.OUT/sys.argv[1]
ray_names=set(json.loads((folder/'ray-probe-names.json').read_text())) if (folder/'ray-probe-names.json').exists() else set()
overrides=json.loads((folder/'expected-overrides.json').read_text()) if (folder/'expected-overrides.json').exists() else {}
records=json.loads((folder/'records.json').read_text())['items']
groups=json.loads((folder/'rvm.json').read_text())
cached_attrs=json.loads((folder/'all-attributes.json').read_text()) if (folder/'all-attributes.json').exists() else None
rows=([dict(id='pe:'+a['refno'].replace('/','_'),name=a['name']) for a in cached_attrs] if cached_attrs is not None else e.sql("SELECT * FROM pe WHERE string::starts_with(name,'/CODEX_TYPES_');")[0])
byid={r['id'].split(':',1)[1]:r for r in rows}
results=[]
for r in records:
    pe=byid.get(r['refno'])
    if pe is None:continue
    name=pe['name'];match=[g for g in groups if g['name']==name and g['geometries']]
    if len(match)!=1:results.append(dict(name=name,error='RVM identity missing',passed=False));continue
    inst=r['insts'][0];out=folder/(r['refno']+'-mesh.json')
    subprocess.run(['D:/Rust/target/debug/examples/pipe_mesh_dump.exe',str(e.REPO/'output/rocksdb-8000-20260905/meshes'/(inst['geo_hash']+'.mesh')),str(out)],check=True,stdout=subprocess.DEVNULL)
    mesh=json.loads(out.read_text());world=matrix(r['world_trans']);vertices=apply(np.array(mesh['vertices']),world@matrix(inst['transform']))
    inv=np.linalg.inv(world);vlocal=apply(vertices,inv)
    plant=[[vlocal[t]] for t in np.array(mesh['indices']).reshape(-1,3)]
    rvm=[[apply(np.array(ring),inv) for ring in poly] for geo in match[0]['geometries'] for poly in geo['polygons_world_mm']]
    rv=volume(rvm);pv=volume(plant)
    signed_rvm_volume=rv
    if any(name in [P+n,P+n+'_UPDATED'] for n in ['PYRA']):
        # These fixtures are convex. RVM primitive tessellation can have mixed
        # winding; tetrahedra about an interior point avoid signed cancellation.
        center=np.concatenate([r for p in rvm for r in p]).mean(0)
        rv=sum(abs(float(np.dot(r[0]-center,np.cross(r[i]-center,r[i+1]-center))))/6 for p in rvm for r in p for i in range(1,len(r)-1))
    err=abs(pv-rv)/max(rv,1)
    pts=np.concatenate([ring for poly in rvm for ring in poly]);bounds=max(abs(np.r_[pts.min(0),pts.max(0)]-np.r_[vlocal.min(0),vlocal.max(0)]))
    results.append(dict(name=name,refno=r['refno'],mesh=inst['geo_hash'],plant_volume=pv,rvm_volume=rv,signed_rvm_volume=signed_rvm_volume,relative_volume_error=err,bounds_error_mm=float(bounds),passed=bool(err<.01 and bounds<2.0)))
    if 'HOST_' in name or name in ray_names:
        probes=[]
        for x in [-473,-227,13,173,347,509]:
            for y in [-431,-193,29,211,389,523]:
                a,b=ray_hits(plant,x,y),ray_hits(rvm,x,y)
                probes.append(dict(x=x,y=y,plant=a,rvm=b,passed=len(a)==len(b) and all(abs(u-v)<2 for u,v in zip(a,b))))
        results[-1]['ray_probes']=probes
        results[-1]['passed']=results[-1]['passed'] and all(p['passed'] for p in probes)
attrs=[]
for pe in ([] if cached_attrs is not None else rows):
    if pe.get('deleted'):continue
    ref=pe['id'].split(':')[1].replace('_','/')
    direct=e.api('element/attributes',{'refno':ref})
    durable=e.sql('SELECT * FROM '+pe['refno']+';')[0]
    attrs.append(dict(name=pe['name'],refno=ref,direct=direct,durable=durable))
if cached_attrs is not None:attrs=cached_attrs
else:e.save(folder/'all-attributes.json',attrs)
result=dict(tolerance=dict(relative_volume=.01,bounds_mm=2),comparisons=results,passed=bool(results) and all(r['passed'] for r in results))
expected_names={g['name'] for g in groups if g['name'].startswith(P) and g['geometries']}
result['missing_models']=sorted(expected_names-{r['name'] for r in results})
result['passed']=result['passed'] and not result['missing_models']
data_checks=[]
selected_nouns=set(sys.argv[2].split(',')) if len(sys.argv)>2 else None
for index,spec in enumerate(specs()):
    if selected_nouns is not None and spec['noun'] not in selected_nouns:continue
    updated=any(a['name']==P+spec['noun']+'_UPDATED' for a in attrs)
    name=P+spec['noun']+('_UPDATED' if updated else '')
    entry=next((a for a in attrs if a['name']==name),None)
    if entry is None:data_checks.append(dict(name=name,passed=False,reason='missing attributes'));continue
    direct={a['name']:a['value'] for a in entry['direct']['attributes']}
    durable=entry['durable'][0] if entry['durable'] else {}
    expected=dict(spec['attrs'])
    if 'resized' in folder.name or updated:expected[spec['attr']]=spec['value']
    if folder.name.startswith('profiles-') and updated and spec['noun'] in ['RTOR','NRTO','CTOR','NCTO']:
        expected['ANGL']=180;expected['ROUT']=520
    expected['NAME']=name
    expected['POS']=[120 if updated else 0,0,0] if spec['negative'] else [(index%4)*2200+(120 if updated else 0),(index//4)*2200,0]
    expected.update(overrides.get(spec['noun'],{}))
    checks={k:dict(expected=v,direct=direct.get(k),durable=durable.get(k),passed=direct.get(k)==v and durable.get(k)==v) for k,v in expected.items()}
    data_checks.append(dict(name=name,checks=checks,passed=all(c['passed'] for c in checks.values())))
result['data_checks']=data_checks
if folder.name=='profiles-moved-result' or selected_nouns is not None:
    for i,noun in enumerate(['EXTR','NXTR','REVO','NREV','POLYHE','NPOLYH','POHE']):
        if selected_nouns is not None and noun not in selected_nouns:continue
        updated=any(a['name']==P+noun+'_UPDATED' for a in attrs)
        name=P+noun+('_UPDATED' if updated else '')
        entry=next((a for a in attrs if a['name']==name),None)
        if entry is None:data_checks.append(dict(name=name,passed=False,reason='missing attributes'));continue
        direct={a['name']:a['value'] for a in entry['direct']['attributes']};durable=entry['durable'][0]
        moved=120 if updated else 0
        resized=updated or 'resized' in folder.name
        expected={'NAME':name,'POS':([moved,0,0] if noun.startswith('N') else [i*2200+moved,11000,0])}
        if noun in ['EXTR','NXTR']:expected['HEIG']=700 if resized else 500
        elif 'REV' in noun:expected['ANGL']=180 if resized else 270
        checks={k:dict(expected=v,direct=direct.get(k),durable=durable.get(k),passed=direct.get(k)==v and durable.get(k)==v) for k,v in expected.items()}
        data_checks.append(dict(name=name,checks=checks,passed=all(c['passed'] for c in checks.values())))
    for noun in ['POLYHE','NPOLYH']:
        if selected_nouns is not None and noun not in selected_nouns:continue
        vertex_x=400 if 'resized' in folder.name or any(a['name']==P+noun+'_UPDATED' for a in attrs) else 300
        for vertex,y,z in [(1,-300,-300),(2,300,-300),(5,-300,300),(6,300,300)]:
            name=P+noun+'_V'+str(vertex);entry=next(a for a in attrs if a['name']==name)
            direct={a['name']:a['value'] for a in entry['direct']['attributes']}
            data_checks.append(dict(name=name,passed=direct.get('POS')==entry['durable'][0].get('POS')==[vertex_x,y,z]))
explicit_expected=folder/'expected-attributes.json'
if explicit_expected.exists():
    for name,expected in json.loads(explicit_expected.read_text()).items():
        entry=next((a for a in attrs if a['name']==name),None)
        direct={a['name']:a['value'] for a in entry['direct']['attributes']} if entry else {}
        durable=entry['durable'][0] if entry and entry['durable'] else {}
        checks={k:dict(expected=v,direct=direct.get(k),durable=durable.get(k),passed=direct.get(k)==v and durable.get(k)==v) for k,v in expected.items()}
        data_checks.append(dict(name=name,checks=checks,passed=bool(checks) and all(c['passed'] for c in checks.values())))
result['data_passed']=bool(data_checks) and all(c['passed'] for c in data_checks)
if len(sys.argv)>3:
    expected_vertices=json.loads((e.OUT/sys.argv[3]/'expected-vertices.json').read_text())
    for expected in expected_vertices:
        entry=next((a for a in attrs if a['refno']==expected['refno']),None)
        if entry is None:data_checks.append(dict(name=expected['name'],passed=False,reason='missing vertex'));continue
        direct={a['name']:a['value'] for a in entry['direct']['attributes']}
        durable=entry['durable'][0]
        data_checks.append(dict(name=expected['name'],passed=direct.get('POS')==durable.get('POS')==expected['expected'] and direct.get('NAME')==durable.get('NAME')==expected['name']))
    result['data_passed']=all(c['passed'] for c in data_checks)
result['geometry_passed']=result['passed']
result['passed']=result['geometry_passed'] and result['data_passed']
result['requested_nouns']=sorted(selected_nouns) if selected_nouns is not None else None
e.save(folder/'types-comparison.json',result)
print(json.dumps(result));
