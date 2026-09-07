"""Read-only source baseline capture. Running service may generate its model cache.

Uses existing E3D launcher, gen_ams and rvm_facets_dump. Never mutates E3D.
Every child exit, model failure, session change and comparison failure is fatal.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import urllib.request
from verify import compare_tree, compare_facets, compare_runtime, load

ROOT = '24384/26229'


def api(base, route, body=None):
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(base.rstrip('/') + '/api/v1/' + route, data=data,
                                    headers={'Content-Type':'application/json'})
    with urllib.request.urlopen(request, timeout=120) as response:
        return json.load(response)


def sha(path):
    with open(path, 'rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--service', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--tools', default='D:/work/plant-code/old/target-e3d-model/release')
    p.add_argument('--db', default='D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams8000_0001')
    p.add_argument('--template', default='E:/reverse/e3d/shadow_e3d31_aps_all')
    p.add_argument('--catalogue', action='append', required=True,
                   help='Exact MDB-resolved catalogue files; repeat for 7000 and 7600')
    p.add_argument('--project-dir', action='append', default=[])
    args = p.parse_args()
    repo = Path(__file__).resolve().parents[2]
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (out/name).write_text(json.dumps(value, indent=2, ensure_ascii=False), encoding='utf-8')
    def run(name, command):
        with (out/(name+'.log')).open('w', encoding='utf-8') as log:
            result = subprocess.run(command, cwd=repo, stdout=log, stderr=subprocess.STDOUT, timeout=180)
        save(name+'-command.json', dict(command=command, exit_code=result.returncode))
        if result.returncode:
            raise RuntimeError(f'{name}: exit {result.returncode}')
    inputs = [Path(args.db), *map(Path, args.catalogue)]
    hashes = {str(file):sha(file) for file in inputs}
    save('input-sha256.json', hashes)
    attributes = api(args.service, 'element/attributes', {'refno':ROOT})
    save('attributes.json', attributes)
    if attributes.get('dbnum') != 8000 or attributes.get('refno') != ROOT:
        raise ValueError('wrong source identity')
    session = attributes['sesno']
    tree = api(args.service, 'tree/children?refno='+ROOT)
    save('tree.json', tree)
    prefix = out.as_posix()
    source = '\n'.join([
        f'ALPHA LOG "{prefix}/tty.log" OVER', '$P PIPE26229-ALIVE', '='+ROOT,
        'Q REF', 'Q NAME', 'Q MEMB', 'repre lev 6', 'repre insu off', 'repre obst off',
        'repre tube on', 'export implied tube into separate containers',
        'export system /expdri.so', f'export file "{prefix}/baseline.rvm"',
        'export holes on', 'export autocolour displayexport on', 'export repr on',
        'export CE', 'export finish', '$P PIPE26229-DONE', 'ALPHA LOG END', 'QUIT', ''])
    (out/'baseline.mac').write_text(source, encoding='utf-8')
    run('tty', [str(repo/'scripts/e3d/run_ams_c_entrymacro.bat'), str(out/'baseline.mac')])
    tty = (out/'tty.log').read_text(encoding='utf-8-sig')
    if 'PIPE26229-DONE' not in tty or not (out/'baseline.rvm').is_file():
        raise ValueError('TTY did not complete export')
    members = compare_tree(tty, tree['nodes'], ROOT)
    command = [str(Path(args.tools)/'gen_ams.exe'), '--attlib',str(Path(args.template)/'attlib.dat'),
               '--db', args.db, '--template', args.template, '--root', ROOT,
               '--sesno',str(session), '--out',str(out/'full')]
    for catalogue in args.catalogue:
        command += ['--catalogue',catalogue]
    for directory in args.project_dir:
        command += ['--project-dir',directory]
    run('full', command)
    generation = load(out/'full/report.json')
    if generation.get('failed'):
        raise ValueError(f"generation failures: {generation['failed']}")
    run('facets', [str(Path(args.tools)/'examples/rvm_facets_dump.exe'), '--rvm',str(out/'baseline.rvm'),
                   '--contains','', '--out',str(out/'rvm-all.json')])
    ensure = api(args.service, 'model/ensure', {'refno':ROOT})
    save('ensure.json', ensure)
    records = api(args.service, 'model/records', {'generation_root':ROOT})
    save('records.json', records)
    end = api(args.service, 'element/attributes', {'refno':ROOT})
    save('attributes-after.json', end)
    after_hashes = {str(file):sha(file) for file in inputs}
    save('input-sha256-after.json', after_hashes)
    if end['sesno'] != session or hashes != after_hashes:
        raise ValueError('source changed during capture; discard mixed-session evidence')
    comparison = compare_facets(load(out/'rvm-all.json'), load(out/'full/elements.json'))
    save('rvm-world-aabb-compare.json', comparison)
    runtime_error = compare_runtime(records, load(out/'full/elements.json'))
    save('summary.json', dict(root=ROOT, sesno=session, members=members, rvm_passed=comparison['passed'],
                             runtime_max_aabb_error_mm=runtime_error,
                             attributes_complete=attributes.get('complete'), incremental='not_run'))
    if not comparison['passed']:
        raise ValueError('RVM comparison failed; see rvm-world-aabb-compare.json')
    if attributes.get('complete') is not True:
        raise ValueError('attribute interface reports incomplete data')


if __name__ == '__main__': main()
