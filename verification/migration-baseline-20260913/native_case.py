"""Native temporary BOX lifecycle. Never restores a live database by overwriting it."""
from pathlib import Path
import json
import re
import shutil
import subprocess
import uuid

def run_native_case(repo, out, env, request):
    folder = out / 'native'
    folder.mkdir()
    target = Path('D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams8000_0001')
    shutil.copy2(target, folder / 'ams8000.before')
    name = '/GMREF_' + uuid.uuid4().hex[:16].upper()
    driver = repo.parent / 'gen-model-refactor-target/debug/l3_suite.exe'
    report = {'name': name, 'phases': [], 'restored': False}
    def macro(label, commands, save=False):
        path = folder / (label + '.mac')
        path.write_text(f'ALPHA LOG "{path.with_suffix(".log").as_posix()}" OVER\n'
                        f'$P GMREF-{label}-BEGIN\n' + commands +
                        (f"\nSAVEWORK 'GMREF {label}'\n" if save else '\n') +
                        f'$P GMREF-{label}-DONE\nALPHA LOG END\n', encoding='utf-8')
        cmd = [str(driver), '--check-driver', str(path), '--project-dir', str(target.parent.parent),
               '--alive-timeout-secs', '60', '--output', str(folder / label)]
        if save: cmd += ['--target-db-file', str(target), '--aios-project', 'AvevaMarineSample']
        with (folder / (label + '-command.log')).open('wb') as log:
            run = subprocess.run(cmd, cwd=repo, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=300)
        report['phases'].append({'label': label, 'command': cmd, 'exit_code': run.returncode})
        text = path.with_suffix('.log').read_text(encoding='utf-8-sig', errors='replace')
        assert run.returncode == 0 and f'GMREF-{label}-DONE' in text, (label, text)
        return text
    def ensure(refno):
        response = request('POST', 'model/ensure', {'refno': refno, 'force': True})
        assert response['status'] == 200, response
        roots = response['body']['generation_roots']
        assert len(roots) == 1, response
        return roots[0]
    def query(refno):
        result = request('POST', 'element/attributes', {'refno':refno})
        assert result['status'] == 200, result
        return result
    previous = request('POST', 'element/attributes', {'refno':'24384/26464'})
    assert previous['status'] == 404, previous
    report['previous_temporary_deleted'] = True
    original_root = ensure('24384/24776')
    original = request('POST', 'model/records', {'generation_root':original_root, 'limit':5000})
    assert original['status'] == 200 and original['body']['complete'] and not original['body']['truncated'], original
    refno = None
    try:
        text = macro('create', f'/1-LNR-Q005-PJ\nNEW BOX {name}\nXLEN 1000\nYLEN 1000\nZLEN 1000\nQ NAME\nQ REF\nQ OWNE\nQ XLEN', True)
        refs = re.findall(r'Ref\s*=\s*(\d+/\d+)', text, re.IGNORECASE)
        assert len(refs) == 1 and name in text, text
        refno = refs[0]
        report['refno'] = refno
        first = query(refno)
        root = ensure(refno)
        before = request('POST', 'model/records', {'generation_root':root, 'limit':10000})
        assert before['status'] == 200, before
        changed = macro('resize', f'{name}\nQ NAME\nQ REF\nXLEN 1500\nQ XLEN', True)
        assert '1500' in changed, changed
        second = query(refno)
        assert first['body'] != second['body'], 'attributes did not refresh'
        assert ensure(refno) == root
        after = request('POST', 'model/records', {'generation_root':root, 'limit':10000})
        assert after['status'] == 200 and before['body']['items'] != after['body']['items'], 'geometry did not refresh'
        report['model_root'] = root
    finally:
        if refno:
            # A separate fresh-session probe confirms identity before deletion.
            proof = macro('cleanup-probe', f'{name}\nQ NAME\nQ REF\nQ TYPE')
            assert name in proof and refno in proof and 'BOX' in proof.upper(), proof
            macro('delete', f'{name}\nDELETE BOX', True)
            absent = request('POST', 'element/attributes', {'refno':refno})
            report['deleted_status'] = absent['status']
            (folder / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
            assert absent['status'] == 404, absent
            report['restored'] = True
            assert ensure(original_root) == original_root
            restored = request('POST', 'model/records', {'generation_root':original_root, 'limit':5000})
            assert restored['status'] == 200, restored
            assert restored['body']['items'] == original['body']['items'], 'original model geometry not restored'
            report['original_geometry_restored'] = True
        (folder / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    return report
