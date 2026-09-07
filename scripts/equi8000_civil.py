"""Civil fixtures in the private AMS 8000 copy, matching existing source ownership."""
import sys
import equi8000_live as e
phase=sys.argv[1]
folder=e.OUT/('civil-'+phase);folder.mkdir(exist_ok=False)
if phase=='create':
    assert not e.sql("SELECT VALUE id FROM pe WHERE name='/CODEX_TYPES_CIVIL_STRU' AND deleted!=true;")[0]
    commands=['='+e.ROOT,'OWNER','NEW STRU /CODEX_TYPES_CIVIL_STRU',
        'NEW FRMW /CODEX_TYPES_CIVIL_FRMW']
    for noun,container,x in [('FLOOR','CFLOOR',0),('GWALL','CWALL',2200)]:
        commands += ['/CODEX_TYPES_CIVIL_FRMW',f'NEW {container} /CODEX_TYPES_{container}',
            f'NEW {noun} /CODEX_TYPES_{noun}',f'POS E {x} N 0 U 0',
            f'NEW PLOO /CODEX_TYPES_{noun}_LOOP','HEIG 200']
        for i,(px,py) in enumerate([(-600,-500),(600,-500),(600,500),(-600,500)]):
            commands += [f'NEW PAVE /CODEX_TYPES_{noun}_V{i}',f'POS E {px} N {py} U 0']
        commands += [f'/CODEX_TYPES_{noun}','Q REF','Q ATT','Q MEMB']
    commands += ["SAVEWORK 'Civil floor and wall fixture'"]
elif phase=='screed-create':
    commands=['/CODEX_TYPES_CIVIL_FRMW','NEW SCREED /CODEX_TYPES_SCREED',
        'POS E 4400 N 0 U 0','NEW PLOO /CODEX_TYPES_SCREED_LOOP','HEIG 200']
    for i,(px,py) in enumerate([(-600,-500),(600,-500),(600,500),(-600,500)]):
        commands += [f'NEW PAVE /CODEX_TYPES_SCREED_V{i}',f'POS E {px} N {py} U 0']
    commands += ['/CODEX_TYPES_SCREED','Q REF','Q ATT','Q MEMB',"SAVEWORK 'Civil screed fixture'"]
elif phase in ('screed-resize','screed-move'):
    commands=(['/CODEX_TYPES_SCREED_LOOP','HEIG 350'] if phase=='screed-resize' else
        ['/CODEX_TYPES_SCREED','BY E 120','NAME /CODEX_TYPES_SCREED_UPDATED'])
    commands += [f"SAVEWORK 'Civil {phase} increment'"]
elif phase in ('holes-create','holes-resize'):
    commands=[]
    for noun in ['FLOOR','GWALL','SCREED']:
        if phase=='holes-create':
            commands += [f'/CODEX_TYPES_{noun}_UPDATED',f'NEW NBOX /CODEX_TYPES_{noun}_CUT',
                'XLEN 300','YLEN 400','ZLEN 1000','POS E 0 N 0 U 175']
        else:commands += [f'/CODEX_TYPES_{noun}_CUT','XLEN 500']
    commands += [f"SAVEWORK 'Civil {phase} increment'"]
elif phase in ('resize','move'):
    commands=[]
    for noun in ['FLOOR','GWALL']:
        if phase=='resize':commands += [f'/CODEX_TYPES_{noun}_LOOP','HEIG 350']
        else:commands += [f'/CODEX_TYPES_{noun}','BY E 120',f'NAME /CODEX_TYPES_{noun}_UPDATED']
    commands += [f"SAVEWORK 'Civil {phase} increment'"]
else:raise ValueError(phase)
e.tty(folder,commands)
print((folder/'tty.log').read_text(encoding='utf-8-sig',errors='replace'))
