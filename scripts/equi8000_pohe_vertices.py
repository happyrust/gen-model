"""Edit legacy POHE vertices by verified references, avoiding TTY navigation."""
import sys
import equi8000_live as e

root=e.api('tree/children?refno='+e.ROOT)
pohe=next(n for n in root['nodes'] if n['name']=='/CODEX_TYPES_POHE_UPDATED')
faces=e.api('tree/children?refno='+pohe['refno'].replace('_','/'))['nodes']
changes=[]
for face in faces:
    for vertex in e.api('tree/children?refno='+face['refno'].replace('_','/'))['nodes']:
        ref=vertex['refno'].replace('_','/')
        attrs=e.api('element/attributes',{'refno':ref})
        pos=next(a['value'] for a in attrs['attributes'] if a['name']=='POS')
        if pos[0]==300:
            changes.append(dict(refno=ref,name='/CODEX_TYPES_POHE_POINT_'+ref.replace('/','_'),expected=[400,pos[1],pos[2]]))
folder=e.OUT/sys.argv[1]
folder.mkdir(exist_ok=False)
e.save(folder/'expected-vertices.json',changes)
assert len(changes)==12, changes
cmd=[]
for c in changes:
    x,y,z=c['expected']
    cmd += ['='+c['refno'],f'POS E {x} N {y} U {z}','NAME '+c['name']]
e.tty(folder,cmd+["SAVEWORK 'POHE referenced vertex increment'"])
print(folder)
