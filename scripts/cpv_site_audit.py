"""Full CPV SITE identity and geometry audit against native E3D RVM."""
from pathlib import Path
import json
import re
import os
import subprocess
import sys
from collections import Counter
import numpy as np

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'output/cpv-site-audit-20260905'
ALIASES = {'CYLINDER':'CYLI','EQUIPMENT':'EQUI','BRANCH':'BRAN','FTUBE':'FTUB',
           'EXTRUSION':'EXTR','TAPER':'TAPE','THREEWAY':'THRE','FITTING':'FITT',
           'SUBEQUIPMENT':'SUBE', 'SUBSTRUCTURE':'SUBS',
           'CTORUS':'CTOR','RTORUS':'RTOR','PYRAMID':'PYRA','PANEL':'PANE',
           'ELBOW':'ELBO','NOZZLE':'NOZZ','FLEXIBLE':'FLEX','SNOUT':'SNOU',
           'GRILLE':'GRIL','SBFITTING':'SBFI','STWALL':'WALL',
           'FRMWORK':'FRMW','STRUCTURE':'STRU'}


def generate():
    env=os.environ.copy()
    env.update(E3D_REPRE_LEVEL_HVAC='6',E3D_REPRE_OBST='off')
    template='E:/reverse/e3d/shadow_e3d31_aps_all'
    args=[str(ROOT.parent/'target-e3d-model/release/gen_ams.exe'),
          '--attlib',template+'/attlib.dat','--template',template,
          '--db','D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams7997_0001',
          '--catalogue','D:/AVEVA/Projects/E3D3.1/ZDJ/ZDJ000/zdj7600_0001',
          '--root','24381/76564','--out',str(OUT/'generated')]
    for folder in ['AvevaMarineSample/ams000','AvevaCatalogue/acp000']:
        args.extend(['--project-dir','D:/AVEVA/Projects/E3D3.1/'+folder])
    with (OUT/'generation.log').open('w') as log:
        subprocess.run(args,env=env,stdout=log,stderr=subprocess.STDOUT,check=True)


def name_key(name):
    name = re.sub(r'\b('+'|'.join(ALIASES)+r')\b',
                  lambda m: ALIASES[m[0]], name)
    name = re.sub(r'\b[A-Z]+\s+(?=/)', '', name)
    # Audits are scoped to one known SITE/ZONE refno. E3D numbers unnamed
    # SITE roots across the MDB, while a direct DB reader numbers within a DB.
    return re.sub(r'SITE \d+ of /\*$', 'SITE of /*', name)


def element_name(e):
    return e['name'] or f"{e['noun']} {e['ordinal']} of {e['owner_desc']}"


def main(elements_path=None, native_path=None, report_path=None):
    elements = json.loads(Path(elements_path or OUT/'generated/elements.json').read_text())
    native = json.loads(Path(native_path or ROOT/'output/strt-cpv-20260905/site-all-rvm.json').read_text())
    by_name = {name_key(element_name(e)):e for e in elements}
    assert len(by_name) == len(elements), 'ambiguous generated names'
    results = []
    for group in native:
        if not group['geometries']:
            continue
        key = name_key(group['name'])
        e = by_name.pop(key, None)
        if e is None:
            results.append(dict(name=key,status='missing'))
            continue
        inv = np.eye(4)
        inv[:3] = np.array(e['world']).T
        inv = np.linalg.inv(inv)
        lows, highs, uncertain = [], [], []
        for g in group['geometries']:
            t = np.eye(4)
            t[:3] = np.array(g['transform_cols_m']).reshape(4,3).T*1000
            t = inv @ t
            if g['kind']=='Cylinder':
                r,h = map(float,re.search(r'radius: ([\d.eE+-]+), height: ([\d.eE+-]+)',g['params']).groups())
                # Zero-radius catalogue bolt placeholders have no surface.
                if r <= 0 or h <= 0:
                    continue
                ext = r*np.sqrt(t[:3,0]**2+t[:3,1]**2)+h/2*abs(t[:3,2])
                lo,hi=t[:3,3]-ext,t[:3,3]+ext
            elif g['kind']=='Snout':
                p={k:float(v) for k,v in re.findall(r'(\w+): ([\d.eE+-]+)',g['params'])}
                if max(p['radius_bottom'],p['radius_top']) <= 0:
                    continue
                ring_bounds=[]
                for sign,end in [(-1,'bottom'),(1,'top')]:
                    radius=p['radius_'+end]
                    centre=np.array([sign*p['offset_x']/2,sign*p['offset_y']/2,sign*p['height']/2])
                    u=np.array([1.,0.,np.tan(p[end+'_shear_x'])])*radius
                    v=np.array([0.,1.,np.tan(p[end+'_shear_y'])])*radius
                    centre=t[:3,:3]@centre+t[:3,3]
                    u,v=t[:3,:3]@u,t[:3,:3]@v
                    ext=np.sqrt(u*u+v*v)
                    ring_bounds.append((centre-ext,centre+ext))
                lo=np.min([b[0] for b in ring_bounds],axis=0)
                hi=np.max([b[1] for b in ring_bounds],axis=0)
            elif g['kind']=='Pyramid':
                m = re.search(r'bottom: (\[[^]]+\]), top: (\[[^]]+\]), offset: (\[[^]]+\]), height: ([\d.eE+-]+)',g['params'])
                bottom,top,offset=[np.array(json.loads(m[i])) for i in [1,2,3]]
                h=float(m[4])
                points=np.array([[sx*d[0]/2+sign*offset[0]/2,sy*d[1]/2+sign*offset[1]/2,sign*h/2]
                                 for sign,d in [(-1,bottom),(1,top)] for sx in [-1,1] for sy in [-1,1]])
                points=points@t[:3,:3].T+t[:3,3]
                lo,hi=points.min(axis=0),points.max(axis=0)
            else:
                points=np.array([p for polygon in g['polygons_world_mm'] or [] for ring in polygon for p in ring])
                if not len(points):
                    uncertain.append(g['kind']); continue
                if g['matches_record_bbox'] is False:
                    uncertain.append(g['kind'])
                points=points@inv[:3,:3].T+inv[:3,3]
                lo,hi=points.min(axis=0),points.max(axis=0)
            lows.append(lo); highs.append(hi)
        if lows:
            expected=np.r_[np.min(lows,axis=0),np.max(highs,axis=0)]
            actual=np.array(e['local_aabb_min']+e['local_aabb_max'])
            error=float(abs(expected-actual).max())
        else:
            expected=actual=np.array([]); error=None
        status='oracle_review' if uncertain else 'bounds_pass' if error is not None and error<=1.1 else 'bounds_fail'
        results.append(dict(name=key,refno=e['refno'],noun=e['noun'],status=status,error_mm=error,
                            uncertain=uncertain,expected=expected.tolist(),actual=actual.tolist()))
    for key,e in by_name.items():
        results.append(dict(name=key,refno=e['refno'],noun=e['noun'],status='extra'))
    report=dict(native_renderable=sum(bool(g['geometries']) for g in native),generated=len(elements),
                counts=dict(Counter(r['status'] for r in results)),results=results)
    report['status'] = 'bounds_complete' if all(r['status']=='bounds_pass' for r in results) else 'incomplete'
    Path(report_path or OUT/'audit.json').write_text(json.dumps(report,indent=2),encoding='utf8')
    print(json.dumps({k:v for k,v in report.items() if k!='results'}))
    print('Failures by noun:',Counter(r.get('noun') for r in results if r['status']=='bounds_fail'))
    for r in sorted([r for r in results if r['status']=='bounds_fail'],key=lambda r:-r['error_mm'])[:12]:
        print(r['refno'],r['name'],round(r['error_mm'],3))
    return report


if __name__=='__main__':
    if '--generate' in sys.argv:
        generate()
    report = main()
    if '--require-complete' in sys.argv and report['status'] != 'bounds_complete':
        raise SystemExit(1)
