"""Compare representation levels and parent export scope for a missing tube."""
import subprocess
import equi8000_live as e
folder=e.OUT/'catalogue-tube-export-probe';folder.mkdir(exist_ok=False)
commands=['repre insu off','repre obst off','repre tube on','export implied tube into separate containers']
for level in [0,6]:
    for scope in ['BRAN','PIPE']:
        label=f'{scope.lower()}-level{level}'
        commands += [f'/CODEX_TYPES_CAT_{scope}',f'repre lev {level}',
          'export system /expdri.so',f'export file "{folder.as_posix()}/{label}.rvm"',
          'export holes on','export autocolour displayexport on','export repr on','export CE','export finish']
e.tty(folder,commands)
for file in folder.glob('*.rvm'):
    subprocess.run(['D:/work/plant-code/old/target-e3d-model/release/examples/rvm_facets_dump.exe',
        '--rvm',str(file),'--contains','','--out',str(file.with_suffix('.json'))],check=True,stdout=subprocess.DEVNULL)
print(folder)
