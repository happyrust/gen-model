"""Fresh private subtree for same-session Plant UI deletion regression."""
import sys, time, struct, urllib.error
import equi8000_live as e

phase = sys.argv[1]
folder = e.OUT / ('ui-delete-' + phase)
folder.mkdir(exist_ok=False)
name = '/CODEX_UI_DELETE_STRU'
if phase == 'create':
    assert not e.sql(f"SELECT VALUE id FROM pe WHERE name='{name}' AND deleted!=true;")[0]
    commands = ['=' + e.ROOT, 'OWNER', 'NEW STRU ' + name,
                'NEW FRMW /CODEX_UI_DELETE_FRMW', 'NEW PANE /CODEX_UI_DELETE_PANE',
                'NEW PLOO /CODEX_UI_DELETE_LOOP', 'HEIG 200']
    for i, (x, y) in enumerate([(-600, -500), (600, -500), (600, 500), (-600, 500)]):
        commands += [f'NEW PAVE /CODEX_UI_DELETE_V{i}', f'POS E {x} N {y} U 0']
    commands += [name, 'Q REF', "SAVEWORK 'UI deletion regression create'"]
else:
    root = e.sql(f"SELECT VALUE id FROM pe WHERE name='{name}' AND deleted!=true;")[0]
    assert len(root) == 1
    ref = root[0].split(':', 1)[1].replace('_', '/')
    todo = [ref]; nodes = []
    while todo:
        item = todo.pop(); nodes.append(item)
        todo.extend(n['refno'].replace('_', '/') for n in e.api('tree/children?refno=' + item)['nodes'])
    e.save(folder / 'before.json', {'root': ref, 'nodes': nodes,
        'models': e.api('model/records', {'generation_root': ref})})
    assert phase == 'delete' and len(nodes) == 8
    commands = [name, 'DELETE STRU', "SAVEWORK 'UI deletion regression delete'"]
e.tty(folder, commands)
data = e.SOURCE.read_bytes()
target = struct.unpack_from('>I', data, struct.unpack_from('>I', data, 40)[0]*2048+12)[0]
deadline = time.monotonic()+150
while e.sql('SELECT VALUE applied_sesno FROM dbnum_watermark:8000;')[0] != [target]:
    assert time.monotonic() < deadline, 'Watcher did not apply source window'
    time.sleep(2)
if phase == 'create':
    rows = e.sql(f"SELECT * FROM pe WHERE name='{name}' AND deleted!=true;")[0]
    assert len(rows) == 1
    e.save(folder / 'result.json', {'session': target, 'root': rows[0]})
else:
    checks = []
    for item in nodes:
        try:
            e.api('element/attributes', {'refno': item}); status = 200
        except urllib.error.HTTPError as error:
            status = error.code
        checks.append({'refno': item, 'status': status})
    models = e.api('model/records', {'generation_root': ref})
    passed = all(c['status'] == 404 for c in checks) and not models['items']
    e.save(folder / 'result.json', {'passed': passed, 'session': target, 'checks': checks, 'models': models})
    assert passed
print((folder / 'result.json').read_text())
