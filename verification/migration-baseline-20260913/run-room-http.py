"""Fresh refactor build and isolated memory server; opt-in native AMS mutation."""
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import time
import urllib.request
import urllib.error

repo = Path(__file__).resolve().parents[2]
out = Path(__file__).resolve().parent / ('http-' + time.strftime('%Y%m%d-%H%M%S'))
out.mkdir()
target = repo.parent / 'gen-model-refactor-target'
cmd = ['cargo', 'build', '--bin', 'aios-database', '--target-dir', str(target), '--message-format=json-render-diagnostics']
with (out / 'build.jsonl').open('wb') as log:
    build = subprocess.run(cmd, cwd=repo, stdout=log, stderr=subprocess.STDOUT)
assert build.returncode == 0, 'build failed: ' + str(out / 'build.jsonl')
artifacts = []
for line in (out / 'build.jsonl').read_text(encoding='utf-8', errors='replace').splitlines():
    try: row = json.loads(line)
    except ValueError: continue
    if row.get('reason') == 'compiler-artifact' and row.get('executable'):
        artifacts.append(Path(row['executable']))
binary = next(p for p in artifacts if p.name == 'aios-database.exe')
shutil.copy2(binary, out / binary.name)
binary = out / binary.name
shutil.copytree(repo / 'resource', out / 'resource')
def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0)); return s.getsockname()[1]
http, store = free_port(), free_port()
assert http != store
config = (repo / 'DbOption.toml').read_text(encoding='utf-8')
rooms = os.environ.get('AMS_SKIP_ROOMS') != '1'
values = {'http_api_addr': f'"127.0.0.1:{http}"', 'server_release_ip': f'"127.0.0.1:{http}"',
          'v_port': str(store), 'surreal_ns': '91613', 'room_membership': str(rooms).lower(),
          'store_mode': '"spawned-mem"', 'startup_autorun': 'false', 'data_face': '"read-through"',
          'meshes_path': json.dumps(str(out / 'assets/meshes').replace('\\', '/'))}
for key, value in values.items():
    pattern = rf'(?m)^{key}\s*=.*$'
    if re.search(pattern, config): config = re.sub(pattern, lambda _: f'{key} = {value}', config)
    else: config = f'{key} = {value}\n' + config
(out / 'DbOption.toml').write_text(config, encoding='utf-8')
env = dict(os.environ, DB_OPTION_FILE=str(out / 'DbOption'), AIOS_OPEN_BROWSER='0',
           AIOS_SURREAL_EXE=str(repo.parent / 'gen-model/bin/surreal.exe'),
           E3D_IO_SCHEMA_ROOT=str(repo.parent / 'vendor/e3d-io/schema'),
           PLANT_ASSET_ROOT=str(out / 'assets'))
native = Path('D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams8000_0001')
def digest(path):
    with path.open('rb') as f: return hashlib.file_digest(f, 'sha256').hexdigest()
before = digest(native)
trace = []
result = {'build_command': cmd, 'build_exit': build.returncode, 'binary': str(binary),
          'binary_sha256': digest(binary), 'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo).decode().strip(),
          'http_port': http, 'source_before': before, 'passed': False}
def request(method, path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(f'http://127.0.0.1:{http}/api/v1/' + path, data=data, method=method,
                                 headers={'Content-Type': 'application/json'})
    start = time.monotonic()
    try: response = urllib.request.urlopen(req, timeout=125)
    except urllib.error.HTTPError as error: response = error
    raw = response.read().decode()
    try: payload = json.loads(raw)
    except ValueError: payload = raw
    item = {'method': method, 'path': path, 'request': body, 'status': response.status,
            'body': payload, 'seconds': time.monotonic() - start}
    trace.append(item)
    return item
def lookup():
    return request('POST', 'query', {'tool': 'e3d.room.lookup', 'arguments': {'refno': '24384/2507'}})
log = (out / 'server.log').open('wb')
process = subprocess.Popen([str(binary)], cwd=out, env=env, stdout=log, stderr=subprocess.STDOUT,
                           creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
result['pid'] = process.pid
(out / 'running.json').write_text(json.dumps(result, indent=2))
try:
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        assert process.poll() is None, 'backend exited'
        try:
            health = request('GET', 'health')
            if health['status'] == 200: break
        except (OSError, urllib.error.URLError): pass
        time.sleep(1)
    else: raise AssertionError('HTTP bind timeout')
    # Wait only on the server's explicit startup gate, never mask arbitrary errors.
    while time.monotonic() < deadline:
        initial = request('POST', 'model/ensure', {'refno': '24384/2507'})
        if initial['status'] == 200: break
        assert initial['status'] in (409, 503), initial
        time.sleep(2)
    else: raise AssertionError('model startup gate did not settle')
    if rooms:
        computed = lookup()
        assert computed['status'] == 200, computed
        value = computed['body']['result']
        assert value['calculation_status'] == 'computed', value
        assert any(edge['room_num'] == 'R320' for edge in value['memberships']), value
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        replies = list(pool.map(lambda _: request('POST', 'model/ensure', {'refno':'24384/2507','force':True}), range(8)))
    assert all(row['status'] == 200 for row in replies), replies
    if rooms:
        after = lookup()
        assert after['status'] == 200 and after['body']['result']['memberships'] == value['memberships'], after
    result['room_checks'] = 'passed' if rooms else 'skipped'
    if os.environ.get('AMS_NATIVE_CRUD') == '1':
        from native_case import run_native_case
        result['native_crud'] = run_native_case(repo, out, env, request)
    result['passed'] = True
except Exception as error:
    result['error'] = repr(error)
finally:
    process.terminate()
    try: process.wait(timeout=20)
    except subprocess.TimeoutExpired: process.kill(); process.wait()
    log.close()
    result['source_after'] = digest(native)
    result['source_unchanged'] = result['source_before'] == result['source_after']
    if os.environ.get('AMS_NATIVE_CRUD') != '1':
        result['passed'] = result['passed'] and result['source_unchanged']
    (out / 'trace.json').write_text(json.dumps(trace, ensure_ascii=False, indent=2), encoding='utf-8')
    (out / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
    print(json.dumps(result, ensure_ascii=False))
    print(out)
raise SystemExit(0 if result['passed'] else 1)
