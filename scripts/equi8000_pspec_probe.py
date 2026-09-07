"""Read and apply a real branch specification in the isolated tube probe."""
import sys
import equi8000_live as e
phase=sys.argv[1]; folder=e.OUT/('catalogue-pspec-'+phase);folder.mkdir(exist_ok=False)
if phase=='read':
    commands=['=24381/573','Q PSPE','OWNER','Q PSPE','/YBS3','Q TYPE','Q REF']
elif phase=='set':
    commands=['/CODEX_TYPES_CAT_BRAN','PSPE /YBS3','Q PSPE',"SAVEWORK 'Tube spec context probe'"]
else:raise ValueError(phase)
e.tty(folder,commands)
print((folder/'tty.log').read_text(encoding='utf-8-sig'))
