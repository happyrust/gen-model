"""Repeatable read-only native E3D captures and model audits outside CPV."""
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
from collections import Counter
import argparse
import json
import os
import subprocess
import traceback
import cpv_site_audit

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT/'output/hvac-sites-audit-20260905'
TARGET = ROOT.parent/'target-e3d-model/release'
TEMPLATE = 'E:/reverse/e3d/shadow_e3d31_aps_all'

def inventory():
    zones = json.loads((OUT/'site-zones.json').read_text())
    selected = []
    for entry in zones:
        site = entry['site']
        if site['name']=='/1CPV-HVACHB':
            continue
        if 'HVAC' in site['name'] or site['refno']=='24381_79977':
            selected.append(dict(refno=site['refno'].replace('_','/'),name=site['name'],site=site['name']))
        if site['name']=='/SSC测试':
            for zone in entry['children']:
                if '暖通' in zone['name']:
                    selected.append(dict(refno=zone['refno'].replace('_','/'),name=zone['name'],site=site['name']))
    (OUT/'targets.json').write_text(json.dumps(selected,indent=2,ensure_ascii=False),encoding='utf-8')
    return selected

def capture(item, folder):
    for name in ['user','work']:
        (folder/name).mkdir(exist_ok=True)
    source=(ROOT/'scripts/e3d/run_ams_c_entrymacro.bat').read_text()
    source=source.replace('set AVEVA_DESIGN_ENTRYMACRO=',
        f"set AVEVA_DESIGN_USER={folder/'user'}\\\nset AVEVA_DESIGN_WORK={folder/'work'}\\\nset AVEVA_DESIGN_ENTRYMACRO=")
    source=source.replace('%~dp0launch_detached.ps1',str(ROOT/'scripts/e3d/launch_detached.ps1'))
    launcher=folder/'run-isolated.bat'
    launcher.write_text(source)
    macro=folder/'export.mac'
    lines=[f'ALPHA LOG "{folder.as_posix()}/tty.log" OVER','$P HVAC-AUDIT-START',
           '='+item['refno'],'Q REF','repre lev 6','repre level hvac 6',
           'repre insu off','repre obst off','repre tube on','Q REPRE',
           'export implied tube into separate containers','export system /expdri.so',
           f'export file "{folder.as_posix()}/native.rvm"','export holes on',
           'export autocolour displayexport on','export repr on','export CE','export finish',
           '$P HVAC-AUDIT-DONE','ALPHA LOG END','QUIT','FINISH','FINISH','']
    macro.write_text('\n'.join(lines),encoding='utf-8')
    env=os.environ.copy();env['L3_E3D_TIMEOUT_SECONDS']='180'
    with (folder/'launch.log').open('w') as log:
        result=subprocess.run([str(launcher),str(macro)],cwd=ROOT,env=env,stdout=log,stderr=subprocess.STDOUT,timeout=210)
    tty=(folder/'tty.log').read_text(encoding='utf-8-sig',errors='replace') if (folder/'tty.log').exists() else ''
    if result.returncode or 'HVAC-AUDIT-DONE' not in tty:
        raise RuntimeError(f'E3D capture failed, exit {result.returncode}; see launch.log / tty.log')
    subprocess.run([str(TARGET/'examples/rvm_facets_dump.exe'),'--rvm',str(folder/'native.rvm'),
                    '--contains','','--out',str(folder/'native.json')],check=True,stdout=subprocess.DEVNULL)

def generate(item,folder):
    db=8000 if item['refno'].startswith('24384/') else 7997
    args=[str(TARGET/'gen_ams.exe'),'--attlib',TEMPLATE+'/attlib.dat','--template',TEMPLATE,
          '--db',f'D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams{db}_0001',
          '--catalogue','D:/AVEVA/Projects/E3D3.1/ZDJ/ZDJ000/zdj7600_0001',
          '--root',item['refno'],'--out',str(folder/'generated')]
    for project in ['AvevaMarineSample/ams000','AvevaCatalogue/acp000']:
        args+=['--project-dir','D:/AVEVA/Projects/E3D3.1/'+project]
    env=os.environ.copy();env.update(E3D_REPRE_LEVEL_HVAC='6',E3D_REPRE_OBST='off')
    with (folder/'generation.log').open('w') as log:
        subprocess.run(args,env=env,stdout=log,stderr=subprocess.STDOUT,check=True)

def run(item, args):
    folder=OUT/item['refno'].replace('/','_');folder.mkdir(exist_ok=True)
    print('START',item['refno'],item['name'],flush=True)
    try:
        with ThreadPoolExecutor(2) as pool:
            jobs=[]
            if args.capture: jobs.append(pool.submit(capture,item,folder))
            if args.generate: jobs.append(pool.submit(generate,item,folder))
            errors=[]
            for job in jobs:
                try: job.result()
                except Exception as error: errors.append(str(error))
            if errors: raise RuntimeError('; '.join(errors))
        report=cpv_site_audit.main(folder/'generated/elements.json',folder/'native.json',folder/'audit.json')
        summary={**item,**{k:v for k,v in report.items() if k!='results'}}
        summary['failures_by_noun']=dict(Counter(r.get('noun','unknown') for r in report['results'] if r['status']=='bounds_fail'))
        return summary
    except Exception as error:
        (folder/'audit-error.txt').write_text(traceback.format_exc(),encoding='utf-8')
        return {**item,'status':'blocked','error':str(error)}

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--capture',action='store_true');parser.add_argument('--generate',action='store_true');parser.add_argument('--only');parser.add_argument('--require-complete',action='store_true')
    args=parser.parse_args()
    targets=inventory()
    if args.only: targets=[t for t in targets if t['refno']==args.only]
    results=[]
    for item in targets:
        result=run(item,args);results.append(result)
        (OUT/('summary-'+args.only.replace('/','_')+'.json' if args.only else 'summary.json')).write_text(json.dumps(results,indent=2,ensure_ascii=False),encoding='utf-8')
        print(json.dumps(result,ensure_ascii=False),flush=True)
    if args.require_complete and any(r['status']!='bounds_complete' for r in results):
        raise SystemExit(1)
