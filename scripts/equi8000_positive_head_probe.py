"""Export a previously documented native head-tube positive control."""
import subprocess
import equi8000_live as e
folder=e.OUT/'catalogue-positive-head';folder.mkdir(exist_ok=False)
e.tty(folder,['/LV-CO-1RX-3062E','Q REF','Q ATT','Q MEMB',
    'repre insu off','repre obst off','repre tube on','export implied tube into separate containers',
    'export system /expdri.so',f'export file "{folder.as_posix()}/model.rvm"',
    'export holes on','export autocolour displayexport on','export repr on','export CE','export finish'])
subprocess.run(['D:/work/plant-code/old/target-e3d-model/release/examples/rvm_facets_dump.exe',
    '--rvm',str(folder/'model.rvm'),'--contains','','--out',str(folder/'rvm.json')],check=True,stdout=subprocess.DEVNULL)
print((folder/'tty.log').read_text(encoding='utf-8-sig'))
