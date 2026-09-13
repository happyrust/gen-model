"""Isolated read-only AMS HTTP probe for the shared MDB read view.

Builds `aios-database` from the current working tree, starts one isolated HTTP
instance (temporary DbOption, ephemeral ports, spawned memory store) and proves:
zero element scans, no repeated opens of an already open database, the
attribute and tree services sharing one open-database set over HTTP, and
byte-identical responses under concurrency.  Every response, every health
counter, the build exit status and the binary SHA-256 are written verbatim to
result.json.  Source database files are only read; their SHA-256 values are
compared before and after the run.  Does not touch run-ams-lazy-http.py.
"""
from pathlib import Path
import concurrent.futures
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request

repo = Path(__file__).resolve().parent.parent
root = Path(os.environ.get('AMS_PROBE_OUTPUT', str(repo / 'verification/ams-shared-read-runtime')))
root.mkdir(parents=True, exist_ok=True)
shutil.copytree(repo / 'resource', root / 'resource', dirs_exist_ok=True)
binary = Path('D:/Rust/target/debug/aios-database.exe')
source_root = Path(os.environ.get('AMS_SOURCE_ROOT', 'D:/AVEVA/Projects/E3D3.1'))
build_command = ['cargo', 'build', '--bin', 'aios-database']

REFNO = '24384/23257'          # BRAN in dbnum 8000 (AMS E3D3.1 /ALL)
ELEMENT_DBNUM = 8000
EXPECTED_ROUTES = 70
EXPECTED_ROOT_OPENS = 28       # element databases that are not open before /tree/roots
CONCURRENCY = 8

result = {'binary': str(binary), 'source_root': str(source_root), 'refno': REFNO,
          'build': {}, 'checks': [], 'steps': []}


def check(name, passed, expected, actual):
    result['checks'].append({'check': name, 'passed': bool(passed),
                             'expected': expected, 'actual': actual})
    return bool(passed)


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, 'rb') as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b''):
            digest.update(chunk)
    return digest.hexdigest()


def hash_tree(tree):
    """SHA-256 of every file below `tree`; unreadable files are recorded, not skipped silently."""
    start = time.monotonic()
    files, errors, total = {}, {}, 0
    for directory, _, names in os.walk(tree):
        for name in names:
            path = Path(directory) / name
            key = path.relative_to(tree).as_posix()
            try:
                files[key] = sha256_file(path)
                total += path.stat().st_size
            except OSError as error:
                errors[key] = repr(error)
    manifest = hashlib.sha256(json.dumps(sorted(files.items())).encode()).hexdigest()
    return {'files': dict(sorted(files.items())), 'unreadable': errors, 'file_count': len(files),
            'total_bytes': total, 'manifest_sha256': manifest,
            'elapsed_ms': round((time.monotonic() - start) * 1000)}


def free_port():
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', 0))
        return probe.getsockname()[1]


# 1. Source files before the run.
before = hash_tree(source_root)
result['source_before'] = {k: before[k] for k in ('file_count', 'total_bytes', 'manifest_sha256',
                                                  'unreadable', 'elapsed_ms')}

# 2. Build the binary from the current working tree; the exit status is evidence.
#    AMS_PROBE_BUILD=0 skips the build (preliminary runs while the tree does not compile);
#    the skip is recorded so the evidence never silently claims a fresh build.
if os.environ.get('AMS_PROBE_BUILD', '1') == '0':
    result['build'] = {'command': build_command, 'skipped': True,
                       'reason': 'AMS_PROBE_BUILD=0: existing binary used without rebuilding'}
else:
    build_start = time.monotonic()
    with (root / 'build.log').open('w', encoding='utf-8') as build_log:
        build = subprocess.run(build_command, cwd=repo, stdout=build_log, stderr=subprocess.STDOUT)
    result['build'] = {'command': build_command, 'cwd': str(repo), 'exit_status': build.returncode,
                       'elapsed_ms': round((time.monotonic() - build_start) * 1000),
                       'log': str(root / 'build.log'),
                       'cargo_target_dir': os.environ.get('CARGO_TARGET_DIR')}
    if build.returncode != 0:
        (root / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        sys.exit(f'cargo build failed: {build.returncode}')
result['sha256'] = sha256_file(binary)
result['binary_size'] = binary.stat().st_size
result['binary_mtime'] = time.strftime('%Y-%m-%dT%H:%M:%S%z', time.localtime(binary.stat().st_mtime))
try:
    result['git'] = {
        'branch': subprocess.run(['git', 'rev-parse', '--abbrev-ref', 'HEAD'], cwd=repo,
                                 capture_output=True, text=True).stdout.strip(),
        'head': subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=repo,
                               capture_output=True, text=True).stdout.strip()}
except OSError as error:
    result['git'] = {'error': repr(error)}

# 3. Isolated configuration: temporary DbOption, ephemeral ports, spawned memory store.
http_port, store_port = free_port(), free_port()
assert http_port != store_port
config = (repo / 'DbOption.toml').read_text(encoding='utf-8')
values = {'surreal_ns': '91517', 'v_port': str(store_port),
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
result.update({'command': [str(binary), 'serve'], 'cwd': str(root),
               'http_port': http_port, 'store_port': store_port})


def request(method, path, body=None):
    """Returns (parsed JSON, raw bytes, elapsed ms). HTTP errors raise with the body."""
    payload = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(f'http://127.0.0.1:{http_port}' + path, data=payload, method=method,
                                 headers={'Content-Type': 'application/json'})
    start = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=120) as response:
            raw = response.read()
    except urllib.error.HTTPError as error:
        raise RuntimeError(f'{method} {path} -> {error.code}: {error.read().decode("utf-8", "replace")}')
    return json.loads(raw), raw, round((time.monotonic() - start) * 1000)


def health():
    value, raw, _ = request('GET', '/api/v1/health')
    return value


def counts(value):
    core = value['core']
    return {'route_count': core['route_count'], 'open_db_count': core['open_db_count'],
            'open_dbnums': core['open_dbnums'], 'cached_elements': core['cached_elements'],
            'source_reads': core['source_reads']}


def opens(value):
    return value['core']['source_reads']['database_opens']


def scans(value):
    return value['core']['source_reads']['scan_calls']


def record(name, method, path, body, value, raw, elapsed, health_before, health_after):
    step = {'step': name, 'method': method, 'path': path, 'body': body, 'elapsed_ms': elapsed,
            'response_sha256': hashlib.sha256(raw).hexdigest(), 'response_bytes': len(raw),
            'response_text': raw.decode('utf-8'),
            'health_before': counts(health_before), 'health_after': counts(health_after),
            'database_opens_delta': opens(health_after) - opens(health_before),
            'open_db_count_delta': health_after['core']['open_db_count'] - health_before['core']['open_db_count'],
            'newly_open_dbnums': sorted(set(health_after['core']['open_dbnums'])
                                        - set(health_before['core']['open_dbnums']))}
    result['steps'].append(step)
    return step


ancestors_path = f'/api/v1/tree/ancestors?refno={REFNO}'
children_path = f'/api/v1/tree/children?refno={REFNO}'
roots_path = '/api/v1/tree/roots'
attributes_body = {'refno': REFNO}
kinds = {'attributes': ('POST', '/api/v1/element/attributes', attributes_body),
         'ancestors': ('GET', ancestors_path, None),
         'children': ('GET', children_path, None)}

with (root / 'stdout.log').open('w', encoding='utf-8') as out, \
     (root / 'stderr.log').open('w', encoding='utf-8') as err:
    start = time.monotonic()
    proc = subprocess.Popen(result['command'], cwd=root, env=env, stdout=out, stderr=err,
                            creationflags=subprocess.CREATE_NO_WINDOW)
    result['pid'] = proc.pid
    try:
        # (a) cold start: the first health with a populated route directory.
        deadline = start + 60
        while True:
            assert proc.poll() is None, f'server exited: {proc.returncode}'
            try:
                cold = health()
                if cold['core']['route_count']:
                    break
            except (OSError, ValueError):
                pass
            assert time.monotonic() < deadline, 'HTTP startup exceeded 60 seconds'
            time.sleep(0.2)
        result['ready_ms'] = round((time.monotonic() - start) * 1000)
        result['cold'] = cold
        result['cold_counts'] = counts(cold)
        check('a.cold.open_db_count == 0', cold['core']['open_db_count'] == 0, 0, cold['core']['open_db_count'])
        check('a.cold.scan_calls == 0', scans(cold) == 0, 0, scans(cold))
        check('a.cold.route_count == 70', cold['core']['route_count'] == EXPECTED_ROUTES,
              EXPECTED_ROUTES, cold['core']['route_count'])

        # Wait for startup background work (dabacon tag sampling) so later deltas are attributable.
        settle_start = time.monotonic()
        stable, baseline = 0, cold
        while time.monotonic() - settle_start < 30:
            current = health()
            refreshing = bool(current.get('dabacon_tags', {}).get('refreshing'))
            stable = stable + 1 if (not refreshing and opens(current) == opens(baseline)) else 0
            baseline = current
            if stable >= 3:
                break
            time.sleep(0.5)
        result['settle_ms'] = round((time.monotonic() - settle_start) * 1000)
        result['settled'] = stable >= 3
        result['baseline_counts'] = counts(baseline)
        check('baseline.open_db_count == 0', baseline['core']['open_db_count'] == 0, 0,
              baseline['core']['open_db_count'])
        check('baseline.scan_calls == 0', scans(baseline) == 0, 0, scans(baseline))

        # (b1) attributes: opens the element database plus its actual dependencies, once.
        serial = {}
        value, raw, elapsed = request(*kinds['attributes'])
        after = health()
        step = record('b1.attributes', *kinds['attributes'], value, raw, elapsed, baseline, after)
        serial['attributes'] = raw
        check('b1.attributes.dbnum == 8000', value.get('dbnum') == ELEMENT_DBNUM, ELEMENT_DBNUM, value.get('dbnum'))
        check('b1.attributes.nonempty', bool(value.get('attributes')), 'non-empty attributes',
              len(value.get('attributes') or []))
        check('b1.attributes.database_opens_delta >= 1', step['database_opens_delta'] >= 1,
              '1 + dependency count', step['database_opens_delta'])
        check('b1.attributes.8000 in open_dbnums', ELEMENT_DBNUM in after['core']['open_dbnums'],
              ELEMENT_DBNUM, after['core']['open_dbnums'])
        check('b1.attributes.scan_calls == 0', scans(after) == 0, 0, scans(after))
        result['attributes_dependency_opens'] = step['database_opens_delta'] - 1
        result['attributes_newly_open_dbnums'] = step['newly_open_dbnums']
        previous = after

        # (b2) ancestors: same library, already open through the attribute service.
        value, raw, elapsed = request(*kinds['ancestors'])
        after = health()
        step = record('b2.ancestors', *kinds['ancestors'], value, raw, elapsed, previous, after)
        serial['ancestors'] = raw
        refnos = value.get('refnos', [])
        check('b2.ancestors.database_opens_delta == 0', step['database_opens_delta'] == 0, 0, step['database_opens_delta'])
        check('b2.ancestors.open_db_count unchanged', step['open_db_count_delta'] == 0, 0, step['open_db_count_delta'])
        check('b2.ancestors.scan_calls == 0', scans(after) == 0, 0, scans(after))
        # Self first, then OWNER chain up to the library WORLD (`<primary_extent_ref>_0`); the WORLD
        # carries the library's primary extent Ref0, so only the chain below it shares Ref0 24384.
        check('b2.ancestors.chain self-first up to WORL', bool(refnos) and refnos[0] == REFNO.replace('/', '_')
              and refnos[-1].endswith('_0') and all(r.startswith('24384_') for r in refnos[:-1]),
              f'{REFNO.replace("/", "_")}, 24384_* ..., <ref0>_0', refnos)
        previous = after

        # (b3) children: same library again.
        value, raw, elapsed = request(*kinds['children'])
        after = health()
        step = record('b3.children', *kinds['children'], value, raw, elapsed, previous, after)
        serial['children'] = raw
        nodes = value.get('nodes', [])
        check('b3.children.database_opens_delta == 0', step['database_opens_delta'] == 0, 0, step['database_opens_delta'])
        check('b3.children.open_db_count unchanged', step['open_db_count_delta'] == 0, 0, step['open_db_count_delta'])
        check('b3.children.scan_calls == 0', scans(after) == 0, 0, scans(after))
        check('b3.children.all dbnum 8000', bool(nodes) and all(n.get('dbnum') == ELEMENT_DBNUM for n in nodes),
              ELEMENT_DBNUM, sorted({n.get('dbnum') for n in nodes}))
        previous = after

        # (c) eight concurrent mixed requests: byte-identical to the serial responses, no new opens.
        order = [list(kinds)[i % len(kinds)] for i in range(CONCURRENCY)]
        with concurrent.futures.ThreadPoolExecutor(max_workers=CONCURRENCY) as pool:
            futures = [pool.submit(request, *kinds[kind]) for kind in order]
            outcomes = [future.result() for future in futures]
        after = health()
        lanes = []
        for lane, (kind, (value, raw, elapsed)) in enumerate(zip(order, outcomes)):
            lanes.append({'lane': lane, 'kind': kind, 'elapsed_ms': elapsed, 'response_bytes': len(raw),
                          'response_sha256': hashlib.sha256(raw).hexdigest(),
                          'identical_bytes': raw == serial[kind],
                          'identical_json': value == json.loads(serial[kind]),
                          'response_text': raw.decode('utf-8')})
        result['steps'].append({'step': 'c.concurrent', 'lanes': lanes,
                                'health_before': counts(previous), 'health_after': counts(after),
                                'database_opens_delta': opens(after) - opens(previous)})
        check('c.concurrent.every lane byte-identical to serial', all(l['identical_bytes'] for l in lanes),
              'identical_bytes for 8 lanes', [(l['kind'], l['identical_bytes']) for l in lanes])
        check('c.concurrent.database_opens unchanged', opens(after) == opens(previous), opens(previous), opens(after))
        check('c.concurrent.scan_calls == 0', scans(after) == 0, 0, scans(after))
        previous = after

        # (d) tree first page: every element database WORLD once; 8000 is already open.
        value, raw, elapsed = request('GET', roots_path)
        after = health()
        step = record('d.roots', 'GET', roots_path, None, value, raw, elapsed, previous, after)
        step['node_count'] = len(value.get('nodes', []))
        step['node_dbnums'] = sorted({n.get('dbnum') for n in value.get('nodes', [])}, key=lambda d: (d is None, d))
        check('d.roots.database_opens_delta == 28', step['database_opens_delta'] == EXPECTED_ROOT_OPENS,
              EXPECTED_ROOT_OPENS, step['database_opens_delta'])
        check('d.roots.scan_calls == 0', scans(after) == 0, 0, scans(after))
        previous = after

        # Informational: a second roots page and a short idle window must not open anything.
        value, raw, elapsed = request('GET', roots_path)
        after = health()
        step = record('extra.roots_repeat', 'GET', roots_path, None, value, raw, elapsed, previous, after)
        step['identical_bytes'] = raw == bytes(result['steps'][-2]['response_text'], 'utf-8')
        time.sleep(2)
        idle = health()
        result['idle_counts'] = counts(idle)
        result['idle_database_opens_delta'] = opens(idle) - opens(after)
        result['final_counts'] = counts(idle)
        result['final_health'] = idle
    except Exception as error:
        result['error'] = repr(error)
        raise
    finally:
        stop = subprocess.run(['taskkill', '/PID', str(proc.pid), '/T', '/F'],
                              capture_output=True, text=True)
        result['stop_exit_status'] = stop.returncode
        result['stop_output'] = stop.stdout + stop.stderr
        try:
            result['process_exit_status'] = proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            result['process_exit_status'] = None
        # (e) source files after the run.
        after_tree = hash_tree(source_root)
        result['source_after'] = {k: after_tree[k] for k in ('file_count', 'total_bytes', 'manifest_sha256',
                                                             'unreadable', 'elapsed_ms')}
        changed = sorted(k for k in set(before['files']) | set(after_tree['files'])
                         if before['files'].get(k) != after_tree['files'].get(k))
        result['source_changed_files'] = changed
        check('e.source files unchanged', not changed and before['manifest_sha256'] == after_tree['manifest_sha256'],
              before['manifest_sha256'], after_tree['manifest_sha256'])
        result['passed'] = 'error' not in result and all(c['passed'] for c in result['checks'])
        (root / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        (root / 'source-hashes-before.json').write_text(json.dumps(before['files'], indent=1), encoding='utf-8')
        (root / 'source-hashes-after.json').write_text(json.dumps(after_tree['files'], indent=1), encoding='utf-8')
        summary = {k: result[k] for k in ('passed', 'ready_ms', 'sha256', 'error') if k in result}
        summary['build_exit_status'] = result['build'].get('exit_status')
        summary['failed_checks'] = [c['check'] for c in result['checks'] if not c['passed']]
        print(json.dumps(summary, ensure_ascii=False))
