from pathlib import Path
import hashlib
import json
import re
import subprocess

root = Path(__file__).resolve().parent.parent
log = (root / 'verification/uda-provider-tests-final.log').read_text(encoding='utf-8')
binary = Path(re.search(r'Running unittests .*?\(([^\r\n]+\.exe)\)', log).group(1))
results = []
for name, args in [
    ('uda-provider-lib-regression', ['--test-threads=1']),
    ('uda-provider-ams-copy', ['ams_dictionary_copy_reload_failure_and_restore', '--ignored', '--nocapture', '--test-threads=1']),
]:
    command = [str(binary), *args]
    with (root / f'verification/{name}.log').open('w', encoding='utf-8') as output:
        run = subprocess.run(command, cwd=root, stdout=output, stderr=subprocess.STDOUT)
    results.append({'command': command, 'exit_status': run.returncode, 'log': name + '.log'})
(root / 'verification/uda-test-commands.json').write_text(json.dumps({
    'binary': str(binary), 'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
    'runs': results}, indent=2), encoding='utf-8')
print(json.dumps(results))
raise SystemExit(any(row['exit_status'] for row in results))
