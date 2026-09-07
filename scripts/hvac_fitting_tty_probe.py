"""Read-only native placement evidence for a section-owned HVAC support fitting."""
from pathlib import Path
import os
import subprocess

root = Path(__file__).resolve().parents[1]
out = root/'output/hvac-flex-gril-fix-20260905'
macro = out/'fitting-placement.mac'
macro.write_text('\n'.join([
    f'ALPHA LOG "{out.as_posix()}/fitting-placement-tty.log" OVER',
    '$P FITTING-PLACEMENT-START',
    '=24381/47187', 'Q NAME', 'Q POSS', 'Q POSE', 'Q BANG', 'Q JUSL',
    '=24381/47188', 'Q NAME', 'Q POSL', 'Q ZDIS', 'Q DELP', 'Q BANG',
    '=13244/270586', 'Q NAME', 'Q GMRE',
    '$P FITTING-PLACEMENT-DONE', 'ALPHA LOG END', 'QUIT', 'FINISH', 'FINISH', ''
]), encoding='utf-8')
env = os.environ.copy()
env['L3_E3D_TIMEOUT_SECONDS'] = '180'
with (out/'fitting-placement-launch.log').open('w') as log:
    result = subprocess.run([
        str(root/'output/hvac-sites-audit-20260905/24381_46775/run-isolated.bat'), str(macro)
    ], cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=210)
text = (out/'fitting-placement-tty.log').read_text(encoding='utf-8-sig', errors='replace')
if result.returncode or 'FITTING-PLACEMENT-DONE' not in text:
    raise RuntimeError('Native fitting placement probe failed; see logs')
print(text)
