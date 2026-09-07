"""Sequential fixture runner; E3D writers must never overlap."""
import subprocess,sys
from pathlib import Path

phase,nouns,suffix=sys.argv[1:4]
profiles={'EXTR','NXTR','REVO','NREV','POLYHE','NPOLYH','POHE'}
for noun in nouns.split(','):
    script='equi8000_profiles.py' if noun in profiles else 'equi8000_types.py'
    args=[sys.executable,str(Path(__file__).with_name(script)),phase,noun,suffix]
    if phase=='create' and noun in {'EXTR','NXTR','REVO','NREV'}:args.append('legacy')
    subprocess.run(args,check=True)
