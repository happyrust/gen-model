"""Run one real civil edit and require automatic publication and independent comparison."""
import json,os,subprocess,sys
import equi8000_live as e
phase=sys.argv[1]
env=os.environ.copy()
def run(script,*args,environment=None):
    subprocess.run([sys.executable,str(e.REPO/'scripts'/script),*args],env=environment,check=True)
run('equi8000_civil.py',phase)
env['EQUI8000_ROOT']='24384/26606'
snapshot='civil-'+phase+'-result'
run('equi8000_live.py','capture',snapshot,environment=env)
folder=e.OUT/snapshot
assert not (folder/'publication-timeout.json').exists(),'Automatic publication timed out; evidence retained'
run('equi8000_civil_expectations.py',snapshot,phase)
with (folder/'comparison.log').open('w') as out:
    subprocess.run([sys.executable,str(e.REPO/'scripts/compare_equi8000_types.py'),snapshot,'FLOOR,GWALL,SCREED'],stdout=out,check=True)
result=json.loads((folder/'types-comparison.json').read_text())
assert result['passed'], 'Geometry/data comparison failed; evidence retained'
print(json.dumps(dict(phase=phase,passed=True,models=len(result['comparisons']),data_fields=sum(len(c['checks']) for c in result['data_checks']),rays=sum(len(c.get('ray_probes',[])) for c in result['comparisons']))),flush=True)
