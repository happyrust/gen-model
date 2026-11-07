"""AMS 8000 equipment increment evidence, with independent E3D RVM exports."""
import json, os, subprocess, sys, urllib.request, hashlib, struct, time, itertools
from pathlib import Path
sys.path.insert(0, str(Path(__file__).parent))
from rocksdb8000_evidence import sql
REPO = Path(__file__).resolve().parents[1]
OUT = REPO/'output/rocksdb-8000-20260905/equi-increment'
OUT.mkdir(exist_ok=True)
ROOT=os.environ.get('EQUI8000_ROOT','24384/24776')
ROOT_RECORD='gen_root:'+ROOT.replace('/','_')
NAME='/CODEX_ROCKS8000_EQ_BOX_20260905'
PROJECTS=REPO/'output/rocksdb-8000-20260905/projects'
SOURCE=PROJECTS/'AvevaMarineSample/ams000/ams8000_0001'
def api(route, body=None):
    req=urllib.request.Request('http://127.0.0.1:18050/api/v1/'+route,
        data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'})
    with urllib.request.urlopen(req,timeout=150) as r: return json.load(r)
def save(path,value): path.write_text(json.dumps(value,indent=2,ensure_ascii=False),encoding='utf-8')
def tty(folder,commands):
    macro=folder/'run.mac'
    macro.write_text('\n'.join([f'ALPHA LOG "{folder.as_posix()}/tty.log" OVER','$P EQUI8000-START',*commands,'$P EQUI8000-DONE','ALPHA LOG END','QUIT','FINISH','FINISH','']),encoding='utf-8')
    env=os.environ.copy(); env['L3_E3D_TIMEOUT_SECONDS']='180'
    env['L3_E3D_PROJECTS_DIR']=str(PROJECTS)
    env['L3_E3D_PROJECT_EVAR']=str(PROJECTS/'AvevaMarineSample/evarsAvevaMarineSample.bat')
    with (folder/'launcher.log').open('w') as log:
        result=subprocess.run([str(REPO/'scripts/e3d/run_ams_c_entrymacro.bat'),str(macro)],cwd=REPO,env=env,stdout=log,stderr=subprocess.STDOUT,timeout=210)
    text=(folder/'tty.log').read_text(encoding='utf-8-sig',errors='replace')
    if result.returncode or 'EQUI8000-DONE' not in text: raise RuntimeError('E3D did not complete; inspect '+str(folder))
if __name__=='__main__':
    if sys.argv[1]=='tree-probe':
        nodes=api('tree/children?refno='+ROOT)['nodes']
        print(json.dumps(nodes))
        assert any(n['name']==NAME for n in nodes), 'Added BOX missing from direct tree after applied window'
        sys.exit(0)
    if sys.argv[1]=='compare':
        folder=OUT/sys.argv[2]
        records=json.loads((folder/'records.json').read_text())['items']
        groups=[g for g in json.loads((folder/'rvm.json').read_text()) if g['geometries']]
        assert len(records)==len(groups), 'RVM/runtime geometry membership count differs'
        pairs=[]
        for row in records:
            match=[g for g in groups if (g['name'].startswith('BOX ') if row['refno']=='24384_24777' else g['name']==NAME)]
            assert len(match)==1, 'RVM/runtime identity ambiguous'
            group=match[0]
            pts=[p for geo in group['geometries'] for poly in geo['polygons_world_mm'] for ring in poly for p in ring]
            expected=[min(p[i] for p in pts) for i in range(3)]+[max(p[i] for p in pts) for i in range(3)]
            actual=row['world_aabb']['mins']+row['world_aabb']['maxs']
            error=max(abs(a-b) for a,b in zip(expected,actual))
            def transform(p,t):
                v=[p[i]*t['scale'][i] for i in range(3)]
                q=t['rotation']; u=q[:3]
                cross=lambda a,b:[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]
                uv=cross(u,v); uuv=cross(u,uv)
                return [v[i]+2*(q[3]*uv[i]+uuv[i])+t['translation'][i] for i in range(3)]
            vertices=[transform(transform(p,row['insts'][0]['transform']),row['world_trans']) for p in itertools.product([-.5,.5],repeat=3)]
            rendered=[min(p[i] for p in vertices) for i in range(3)]+[max(p[i] for p in vertices) for i in range(3)]
            mesh_error=max(abs(a-b) for a,b in zip(expected,rendered))
            pairs.append({'refno':row['refno'],'rvm_name':group['name'],'error_mm':error,'passed':error<=0.01})
            pairs[-1].update(mesh_error_mm=mesh_error,passed=error<=0.01 and mesh_error<=0.01)
        result={'pairs':pairs,'passed':bool(pairs) and all(p['passed'] for p in pairs),'tolerance_mm':0.01,'scope':'per-element world AABB against E3D RVM facets; not a topology proof'}
        save(folder/'comparison.json',result); print(json.dumps(result)); assert result['passed']; sys.exit(0)
    if sys.argv[1]=='observe':
        records=api('model/records',{'generation_root':ROOT})
        state=sql("SELECT applied_sesno,file_latest_sesno FROM dbnum_watermark:8000; SELECT * FROM gen_root:24384_24776;")
        print(json.dumps({'state':state,'items':records['items']},ensure_ascii=False))
        sys.exit(0)
    if sys.argv[1]=='probe':
        receipt=api('model/ensure',{'refno':ROOT})
        records=api('model/records',{'generation_root':ROOT})
        print(json.dumps({'receipt':receipt['status'],'published':receipt.get('published_target'),'items':len(records['items'])}))
        assert records['items'], 'E3D RVM contains BOX but current model records are empty'
        sys.exit(0)
    action, phase=sys.argv[1:3]
    folder=OUT/phase; folder.mkdir(exist_ok=False)
    source_before=SOURCE.read_bytes()
    save(folder/'source-before.json',{'sha256':hashlib.sha256(SOURCE.read_bytes()).hexdigest(),'size':SOURCE.stat().st_size})
    if action in ('capture','capture-now'):
        target=struct.unpack_from('>I',source_before,struct.unpack_from('>I',source_before,40)[0]*2048+12)[0]
        deadline=time.monotonic()+(150 if action=='capture' else 0)
        while True:
            states=sql('SELECT * FROM '+ROOT_RECORD+';')[0]
            state=states[0] if states else {}
            records=api('model/records',{'generation_root':ROOT})
            if state.get('published_target',{}).get('source_end_sesno')==target and len(records['items'])==state.get('published_geometry_count') and records['items']: break
            if time.monotonic()>deadline:
                save(folder/'publication-timeout.json',{'target':target,'state':state,'records':records})
                break
            time.sleep(2)
        save(folder/'attributes.json',api('element/attributes',{'refno':ROOT}))
        save(folder/'tree.json',api('tree/children?refno='+ROOT))
        # Capture the automatic watcher result without an ensure that could mask a missed increment.
        save(folder/'records.json',api('model/records',{'generation_root':ROOT}))
        save(folder/'database.json',sql("SELECT * FROM dbnum_watermark:8000; SELECT * FROM "+ROOT_RECORD+";"))
        tty(folder,['='+ROOT,'Q REF','Q NAME','Q MEMB','repre lev 6','repre insu off','repre obst off','repre tube on',
            'export implied tube into separate containers','export system /expdri.so',f'export file "{folder.as_posix()}/model.rvm"',
            'export holes on','export autocolour displayexport on','export repr on','export CE','export finish'])
        if not (folder/'model.rvm').is_file(): raise RuntimeError('RVM export missing')
        with (folder/'facets.log').open('w') as log:
            subprocess.run(['D:/work/plant-code/old/vendor/e3d-model/target/release/examples/rvm_facets_dump.exe','--rvm',str(folder/'model.rvm'),'--contains','','--out',str(folder/'rvm.json')],stdout=log,stderr=subprocess.STDOUT,check=True)
        source_after=SOURCE.read_bytes()
        changes=[i for i,(a,b) in enumerate(zip(source_before,source_after)) if a!=b]
        save(folder/'source-byte-differences.json',{'before_size':len(source_before),'after_size':len(source_after),'offsets':changes})
        def session(data): return struct.unpack_from('>I',data,struct.unpack_from('>I',data,40)[0]*2048+12)[0]
        assert session(source_before)==session(source_after),'source session changed while capturing'
        assert len(source_before)==len(source_after) and all(i<2048 for i in changes),'source data pages changed during capture'
    elif action=='edit':
        commands={
          'add':['='+ROOT,'NEW BOX '+NAME,'XLEN 500','YLEN 400','ZLEN 300'],
          'resize':[NAME,'XLEN 900'],
          'resize700':[NAME,'XLEN 700'],
          'move':[NAME,'BY E 1000'],
          'restore':[NAME,'DELETE BOX']}
        if phase=='add':
            assert not sql("SELECT VALUE id FROM pe WHERE name='"+NAME+"';")[0], 'Test element already exists'
            (folder/'source-backup').write_bytes(SOURCE.read_bytes())
        tty(folder,commands[phase]+['Q CE','Q REF','SAVEWORK \'RocksDB equipment increment '+phase+'\''])
    save(folder/'source-after.json',{'sha256':hashlib.sha256(SOURCE.read_bytes()).hexdigest(),'size':SOURCE.stat().st_size})
    print(str(folder))
