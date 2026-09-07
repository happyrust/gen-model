"""Compare all visible route solids to RVM, including unnamed members and tubes."""
import json,sys,subprocess,re,math
import numpy as np
import equi8000_live as e

def matrix(t):
    x,y,z,w=t['rotation'];m=np.eye(4)
    m[:3,:3]=np.array([[1-2*(y*y+z*z),2*(x*y-z*w),2*(x*z+y*w)],
                      [2*(x*y+z*w),1-2*(x*x+z*z),2*(y*z-x*w)],
                      [2*(x*z-y*w),2*(y*z+x*w),1-2*(x*x+y*y)]])@np.diag(t['scale'])
    m[:3,3]=t['translation'];return m
def apply(points,m):return (np.c_[points,np.ones(len(points))]@m.T)[:,:3]
def measures(polys):
    points=np.concatenate([r for p in polys for r in p]); center=points.mean(axis=0)
    volume=abs(sum(float(np.dot(r[0]-center,np.cross(r[i]-center,r[i+1]-center)))/6
                   for p in polys for r in p for i in range(1,len(r)-1)))
    return np.r_[points.min(0),points.max(0)],volume

def rvm_primitive_volume(geometry):
    """Closed analytic volume for an RVM primitive.

    Partial torus tessellation omits its end caps, so treating its polygon soup
    as closed under-counts volume. Parameters are also stable across different
    cylinder segment counts. FacetGroup has no analytic form and keeps the mesh
    integral.
    """
    kind = geometry['kind']
    params = geometry.get('params') or ''
    values = {
        name: float(value)
        for name, value in re.findall(
            r'(\w+):\s*(-?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?)', params
        )
    }
    if kind == 'Box':
        lengths = [float(value) for value in re.search(r'lengths:\s*\[([^]]+)\]', params).group(1).split(',')]
        return math.prod(lengths)
    if kind == 'Cylinder':
        return math.pi * values['radius'] ** 2 * abs(values['height'])
    if kind == 'Snout':
        bottom, top, height = values['radius_bottom'], values['radius_top'], abs(values['height'])
        return math.pi * height * (bottom * bottom + bottom * top + top * top) / 3
    if kind == 'CircularTorus':
        return math.pi * values['radius'] ** 2 * values['offset'] * abs(values['angle'])
    if kind == 'RectangularTorus':
        inner, outer = values['inner_radius'], values['outer_radius']
        return 0.5 * (outer * outer - inner * inner) * abs(values['height'] * values['angle'])
    if kind == 'Pyramid':
        bottom = [float(value) for value in re.search(r'bottom:\s*\[([^]]+)\]', params).group(1).split(',')]
        top = [float(value) for value in re.search(r'top:\s*\[([^]]+)\]', params).group(1).split(',')]
        a0, a1 = math.prod(bottom), math.prod(top)
        return abs(values['height']) * (a0 + a1 + math.sqrt(a0 * a1)) / 3
    if kind == 'SphericalDish':
        radius, height = values['base_radius'], abs(values['height'])
        return math.pi * height * (3 * radius * radius + height * height) / 6
    polygons = [[np.array(r) for r in p] for p in geometry['polygons_world_mm']]
    return measures(polygons)[1]

folder=e.OUT/sys.argv[1]
cold_restart = '--cold-restart' in sys.argv[2:]
publication_timeout = (folder/'publication-timeout.json').exists()
assert cold_restart or not publication_timeout, 'Automatic publication did not complete'
records=json.loads((folder/'records.json').read_text())['items']
groups=[g for g in json.loads((folder/'rvm.json').read_text()) if g['geometries']]
baseline=[]
for group in groups:
    polys=[[np.array(r) for r in p] for geo in group['geometries'] for p in geo['polygons_world_mm']]
    bounds,mesh_volume=measures(polys)
    analytic_volume=sum(rvm_primitive_volume(geo) for geo in group['geometries'])
    baseline.append((group['name'],bounds,mesh_volume,analytic_volume,len(group['geometries'])))
checks=[];invalid=[];used=set()
for index,record in enumerate(records):
    if any(i.get('is_invalid_tubi') for i in record['insts']):
        invalid.append(record);continue
    polys=[]
    for inst_index,inst in enumerate(record['insts']):
        out=folder/f'route-{index}-{inst_index}-mesh.json'
        subprocess.run(['D:/Rust/target/debug/examples/pipe_mesh_dump.exe',
            str(e.REPO/'output/rocksdb-8000-20260905/meshes'/(inst['geo_hash']+'.mesh')),str(out)],check=True,stdout=subprocess.DEVNULL)
        mesh=json.loads(out.read_text());verts=apply(np.array(mesh['vertices']),matrix(record['world_trans'])@matrix(inst['transform']))
        polys.extend([[verts[tri]] for tri in np.array(mesh['indices']).reshape(-1,3)])
    bounds,vol=measures(polys)
    candidates=sorted((float(np.max(abs(bounds-b))),i) for i,(_,b,_,_,_) in enumerate(baseline) if i not in used)
    if not candidates:
        checks.append({'refno':record['refno'],'passed':False,'error':'No RVM solid remaining'});continue
    error,match=candidates[0]
    assert len(candidates)==1 or abs(candidates[1][0]-error)>1e-6,'Ambiguous RVM correspondence'
    used.add(match);name,_,mesh_expected,analytic_expected,primitive_count=baseline[match]
    mesh_volume_error=abs(vol-mesh_expected)/max(mesh_expected,1)
    analytic_volume_error=abs(vol-analytic_expected)/max(analytic_expected,1)
    volume_error=min(mesh_volume_error,analytic_volume_error)
    checks.append({'refno':record['refno'],'noun':record['generic'],'rvm_name':name,
        'bounds_error_mm':error,'relative_volume_error':volume_error,
        'rvm_mesh_volume_error':mesh_volume_error,
        'rvm_analytic_volume_error':analytic_volume_error,
        'rvm_primitive_count':primitive_count,
        'passed':error<2 and volume_error<.04})
result={'passed':bool(checks) and all(c['passed'] for c in checks) and len(used)==len(baseline),
        'checks':checks,'missing_rvm_matches':[name for i,(name,_,_,_,_) in enumerate(baseline) if i not in used],
        'invalid_tubi_not_solid_comparison':len(invalid),'rvm_solids':len(baseline),
        'cold_restart_persisted_snapshot':cold_restart,
        'root_watermark_behind_unrelated_file_session':cold_restart and publication_timeout,
        'scope':'all non-invalid solids; world-space bounds plus the tighter of tessellated and analytic RVM volume errors; invalid-tube UI lines need separate validation'}
e.save(folder/'route-comparison.json',result)
print(json.dumps(result));assert result['passed']
