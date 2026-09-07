"""Read catalogue formulas from native E3D, using the existing isolated launcher."""
from pathlib import Path
import os
import subprocess

root = Path(__file__).resolve().parents[1]
out = root / 'output/hvac-flex-gril-fix-20260905'
out.mkdir(exist_ok=True)
macro = out / 'formulas.mac'
macro.write_text('\n'.join([
    f'ALPHA LOG "{out.as_posix()}/formulas-tty.log" OVER',
    '$P FLEX-GRIL-PROBE-START',
    "VAR !probe ( MAT ( 'abc' , 'b' ) )", '$P MATCH=$!probe',
    "VAR !probe ( MAT ( 'false' , 'true' ) )", '$P FALSE-MATCH=$!probe',
    "VAR !probe ( MAT ( TRIM ( STR ( 150 GT 150 ) ) , 'true' ) )", '$P GT-EQUAL=$!probe',
    "VAR !probe ( MAT ( TRIM ( STR ( 151 GT 150 ) ) , 'true' ) )", '$P GT-GREATER=$!probe',
    '=15194/5489', 'Q NAME', 'Q POFF',
    '=15194/22899', 'Q NAME', 'Q PDIA', 'Q PDIS',
    '=15194/22975', 'Q NAME', 'Q PBDM',
    '=15194/3412', 'Q NAME', 'Q PAAX', 'Q PBAX', 'Q PCAX',
    'Q PBDI', 'Q PTDI',
    '$P FLEX-GRIL-PROBE-DONE', 'ALPHA LOG END', 'QUIT', 'FINISH', 'FINISH', ''
]), encoding='utf-8')
env = os.environ.copy()
env['L3_E3D_TIMEOUT_SECONDS'] = '180'
with (out / 'formulas-launch.log').open('w') as stream:
    result = subprocess.run([
        str(root / 'output/hvac-sites-audit-20260905/24381_46775/run-isolated.bat'), str(macro)
    ], cwd=root, env=env, stdout=stream, stderr=subprocess.STDOUT, timeout=210)
text = (out / 'formulas-tty.log').read_text(encoding='utf-8-sig', errors='replace')
if result.returncode or 'FLEX-GRIL-PROBE-DONE' not in text:
    raise RuntimeError('Native formula query failed; see isolated probe logs')
print(text)
