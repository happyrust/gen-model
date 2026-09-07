"""Isolate catalogue selection from route container creation."""
import sys
import equi8000_live as e
phase=sys.argv[1]
folder=e.OUT/('catalogue-valve-'+phase);folder.mkdir(exist_ok=False)
if phase=='create':
    commands=['/CODEX_TYPES_CAT_BRAN','NEW VALV /CODEX_TYPES_CAT_SBOX']
elif phase=='select':
    commands=['/CODEX_TYPES_CAT_SBOX','SPRE /SBCL150RFDD']
elif phase=='select-native':
    commands=['/CODEX_TYPES_CAT_SBOX','SPRE /DR07C/MACB200:10']
elif phase=='resize-native':
    commands=['/CODEX_TYPES_CAT_SBOX','SPRE /DR07C/MACB200:25']
elif phase=='move-native':
    commands=['/CODEX_TYPES_CAT_SBOX','BY E 120','NAME /CODEX_TYPES_CAT_SREV_UPDATED',
              'SHOP TRUE','BUIL TRUE']
elif phase=='align-endpoints':
    commands=['/CODEX_TYPES_CAT_BRAN','HPOS E 120 N 0 U 0','TPOS E 120 N 0 U 0']
elif phase=='explicit-head':
    commands=['/CODEX_TYPES_CAT_BRAN','HPOS E 0 N 0 U 0']
elif phase=='head-bore':
    commands=['/CODEX_TYPES_CAT_BRAN','HBOR 25']
elif phase=='head-stub':
    commands=['/CODEX_TYPES_CAT_BRAN','HSTU /YBS3/CIP00001:P,7']
elif phase=='head-connection':
    commands=['/CODEX_TYPES_CAT_BRAN','HCON OPEN']
elif phase=='position-flag':
    commands=['/CODEX_TYPES_CAT_SREV_UPDATED','POSFLAG TRUE']
else:raise ValueError(phase)
e.tty(folder,commands+['Q REF','Q ATT',"SAVEWORK 'Catalogue valve "+phase+"'"])
print((folder/'tty.log').read_text(encoding='utf-8-sig'))
