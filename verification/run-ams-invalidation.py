"""Repeatable per-database invalidation run against copies of the AMS 8000/7998 files.

Runs `tests/shared_read_view_invalidation.rs` twice: the always-on synthetic two-database
fixture, then the opt-in AMS copy scenario (`-- --ignored`). The test copies `ams8000_0001` and
`ams7998_0001` into a tempdir and damages only the copies; this script hashes the source files
before and after and refuses to report success if either digest moved. Source files are never
opened for writing here.

Outputs (next to this script): `ams-invalidation-run.log` (literal cargo output of both runs)
and `ams-invalidation-result.json` (commands, exit codes, elapsed, digests, counter lines).

Environment: `AIOS_AMS_DB_DIR` overrides the AMS directory (default below); `AIOS_E3D_TEMPLATE_DIR`
overrides the schema directory (default: e3d-io bundled e3d31 pack).
"""
from pathlib import Path
import hashlib
import json
import os
import re
import subprocess
import sys
import time

repo = Path(__file__).resolve().parent.parent
ams_dir = Path(os.environ.get('AIOS_AMS_DB_DIR', r'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000'))
sources = ['ams8000_0001', 'ams7998_0001']
log_path = repo / 'verification/ams-invalidation-run.log'
result_path = repo / 'verification/ams-invalidation-result.json'
test_binary = ['cargo', 'test', '--test', 'shared_read_view_invalidation', '--']
runs = [
    ('synthetic', test_binary + ['--test-threads=1', '--nocapture']),
    ('ams-copies', test_binary + ['--ignored', '--test-threads=1', '--nocapture']),
]


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b''):
            digest.update(chunk)
    return digest.hexdigest()


def snapshot():
    return {name: {'sha256': sha256(ams_dir / name), 'bytes': (ams_dir / name).stat().st_size,
                   'mtime_ns': (ams_dir / name).stat().st_mtime_ns} for name in sources}


for name in sources:
    assert (ams_dir / name).is_file(), f'missing AMS source file: {ams_dir / name}'

result = {'repo': str(repo), 'ams_dir': str(ams_dir), 'sources_before': snapshot(), 'runs': []}
env = dict(os.environ, RUST_BACKTRACE='1')
with log_path.open('w', encoding='utf-8') as log:
    for label, command in runs:
        log.write(f'$ {" ".join(command)}\n')
        log.flush()
        started = time.monotonic()
        proc = subprocess.run(command, cwd=repo, env=env, capture_output=True, text=True,
                              encoding='utf-8', errors='replace')
        elapsed_ms = round((time.monotonic() - started) * 1000)
        log.write(proc.stdout)
        log.write(proc.stderr)
        log.write(f'\n[exit status {proc.returncode}, {elapsed_ms} ms]\n\n')
        summary = re.findall(r'^test result: .*$', proc.stdout, flags=re.M)
        # `--nocapture` prints the first line of a test's output on the `test <name> ...` line itself.
        counters = re.findall(r'(\[(?:synthetic|ams)[^\]]*\] .*)$', proc.stdout, flags=re.M)
        result['runs'].append({'label': label, 'command': command, 'exit_status': proc.returncode,
                               'elapsed_ms': elapsed_ms, 'test_result': summary, 'counters': counters})
        print(json.dumps({'run': label, 'exit_status': proc.returncode, 'elapsed_ms': elapsed_ms,
                          'test_result': summary}, ensure_ascii=False))

result['sources_after'] = snapshot()
result['sources_unchanged'] = all(
    result['sources_before'][name]['sha256'] == result['sources_after'][name]['sha256']
    and result['sources_before'][name]['bytes'] == result['sources_after'][name]['bytes']
    for name in sources)
result['passed'] = result['sources_unchanged'] and all(run['exit_status'] == 0 for run in result['runs'])
result_path.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
print(json.dumps({'passed': result['passed'], 'sources_unchanged': result['sources_unchanged'],
                  'log': str(log_path), 'result': str(result_path)}, ensure_ascii=False))
sys.exit(0 if result['passed'] else 1)
