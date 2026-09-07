"""Delete only the private civil fixture and verify source/data/model removal."""
import struct,time,urllib.error
import equi8000_live as e
root='24384/26606'
folder=e.OUT/'civil-deleted';folder.mkdir(exist_ok=False)
todo=[root];nodes=[]
while todo:
    ref=todo.pop();nodes.append(ref)
    todo.extend(n['refno'].replace('_','/') for n in e.api('tree/children?refno='+ref)['nodes'])
assert len(nodes)>20
before=[]
for ref in nodes:
    key=ref.replace('/','_')
    pe=e.sql('SELECT * FROM pe:'+key+';')[0][0]
    assert pe['name'].startswith('/CODEX_TYPES_')
    before.append(dict(refno=ref,pe=pe,attributes=e.api('element/attributes',{'refno':ref})))
e.save(folder/'before.json',before)
models=e.api('model/records',{'generation_root':root})
assert len(models['items'])==3
e.save(folder/'before-models.json',models)
e.tty(folder,['/CODEX_TYPES_CIVIL_STRU','DELETE STRU',"SAVEWORK 'Remove private civil fixture'"])
data=e.SOURCE.read_bytes();target=struct.unpack_from('>I',data,struct.unpack_from('>I',data,40)[0]*2048+12)[0]
deadline=time.monotonic()+150;observations=[]
while True:
    water=e.sql('SELECT applied_sesno FROM dbnum_watermark:8000;')[0][0]
    models=e.api('model/records',{'generation_root':root})
    observations.append(dict(watermark=water,models=models['total']))
    if water['applied_sesno']==target and not models['items']:break
    if time.monotonic()>deadline:
        e.save(folder/'publication-timeout.json',observations)
        raise RuntimeError('Deletion did not publish automatically')
    time.sleep(2)
checks=[]
for entry in before:
    ref=entry['refno'];pe=e.sql('SELECT * FROM pe:'+ref.replace('/','_')+';')[0]
    try:e.api('element/attributes',{'refno':ref});status=200
    except urllib.error.HTTPError as error:status=error.code
    checks.append(dict(refno=ref,api_status=status,tombstoned=bool(pe) and pe[0].get('deleted') is True))
result=dict(passed=all(c['api_status']==404 and c['tombstoned'] for c in checks),target=target,checks=checks,observations=observations,remaining_models=models['total'])
e.save(folder/'result.json',result)
assert result['passed']
print(dict(passed=True,deleted_nodes=len(checks),session=target,remaining_models=models['total']))
