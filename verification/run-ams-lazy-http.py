"""Isolated read-only AMS HTTP probe. Does not modify source database files."""
from pathlib import Path
import concurrent.futures
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import time
import urllib.request

repo = Path(__file__).resolve().parent.parent
root = Path(os.environ.get('AMS_PROBE_OUTPUT', str(repo / 'verification/ams-lazy-runtime')))
root.mkdir(exist_ok=True)
shutil.copytree(repo / 'resource', root / 'resource', dirs_exist_ok=True)
binary = Path('D:/Rust/target/debug/aios-database.exe')
def free_port():
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', 0))
        return probe.getsockname()[1]

http_port, store_port = free_port(), free_port()
assert http_port != store_port
config = (repo / 'DbOption.toml').read_text(encoding='utf-8')
values = {'surreal_ns': '91516', 'v_port': str(store_port),
          'http_api_addr': f'"127.0.0.1:{http_port}"',
          'server_release_ip': f'"127.0.0.1:{http_port}"',
          'startup_autorun': 'false', 'room_membership': 'false'}
config = re.sub(r'(?m)^(' + '|'.join(values) + r')\s*=.*$',
                lambda m: m[1] + ' = ' + values[m[1]], config)
(root / 'DbOption.toml').write_text(config, encoding='utf-8')
for port in (http_port, store_port):
    with socket.socket() as probe:
        assert probe.connect_ex(('127.0.0.1', port)) != 0, f'port {port} is in use'

env = dict(os.environ, DB_OPTION_FILE=str(root / 'DbOption.toml'),
           AIOS_SURREAL_EXE='D:/work/plant-code/old/gen-model/bin/surreal.exe',
           AIOS_E3D_SCHEMA_ROOT=str(repo.parent / 'vendor/e3d-io/schema'),
           AIOS_OPEN_BROWSER='0', PLANT_ASSET_ROOT=str(root / 'assets'))
result = {'binary': str(binary), 'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
          'command': [str(binary), 'serve'], 'cwd': str(root),
          'http_port': http_port, 'store_port': store_port, 'requests': []}

def request(path, body=None):
    payload = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(f'http://127.0.0.1:{http_port}' + path, data=payload,
                                headers={'Content-Type': 'application/json'})
    start = time.monotonic()
    with urllib.request.urlopen(req, timeout=30) as response:
        value = json.load(response)
    return value, round((time.monotonic() - start) * 1000)

with (root / 'stdout.log').open('w', encoding='utf-8') as out, \
     (root / 'stderr.log').open('w', encoding='utf-8') as err:
    start = time.monotonic()
    proc = subprocess.Popen(result['command'], cwd=root, env=env, stdout=out, stderr=err,
                            creationflags=subprocess.CREATE_NO_WINDOW)
    result['pid'] = proc.pid
    try:
        deadline = start + 60
        while True:
            assert proc.poll() is None, f'server exited: {proc.returncode}'
            try:
                cold, _ = request('/api/v1/health')
                if cold['core']['route_count']:
                    break
            except (OSError, ValueError):
                pass
            assert time.monotonic() < deadline, 'HTTP startup exceeded 60 seconds'
            time.sleep(0.2)
        result['ready_ms'] = round((time.monotonic() - start) * 1000)
        result['cold'] = cold
        assert cold['core']['open_db_count'] == 0, cold['core']
        assert cold['core']['source_reads']['scan_calls'] == 0, cold['core']
        assert cold['core']['route_count'] == 70, cold['core']
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            futures = [pool.submit(request, '/api/v1/element/attributes',
                                   {'refno': '24384/23257'}) for _ in range(8)]
            values = [future.result() for future in futures]
        result['attributes'] = [{'response': value, 'elapsed_ms': elapsed} for value, elapsed in values]
        assert all(value['dbnum'] == 8000 and value['attributes'] for value, _ in values)
        assert all(value == values[0][0] for value, _ in values)
        result['after_attributes'], _ = request('/api/v1/health')
        repeat, result['repeat_ms'] = request('/api/v1/element/attributes', {'refno': '24384/23257'})
        assert repeat == values[0][0]
        result['after_repeat'], _ = request('/api/v1/health')
        assert result['after_repeat']['core']['source_reads']['database_opens'] == \
               result['after_attributes']['core']['source_reads']['database_opens'], \
               'a warm point request reopened unrelated MDB files'
        result['passed'] = True
    except Exception as error:
        result['passed'] = False
        result['error'] = repr(error)
        raise
    finally:
        stop = subprocess.run(['taskkill', '/PID', str(proc.pid), '/T', '/F'],
                              capture_output=True, text=True)
        result['stop_exit_status'] = stop.returncode
        result['stop_output'] = stop.stdout + stop.stderr
        result['process_exit_status'] = proc.wait(timeout=10)
        (root / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps({k: result[k] for k in ('passed', 'ready_ms', 'error') if k in result}, ensure_ascii=False))
