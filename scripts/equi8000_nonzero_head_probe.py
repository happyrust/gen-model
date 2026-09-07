"""Move only the original catalogue fixture head off world zero."""
import sys
import equi8000_live as e
phase=sys.argv[1] if len(sys.argv)>1 else 'nonzero'
commands={'nonzero':'HPOS E 10 N 0 U 0','long':'HPOS W 1000 N 0 U 0',
          'positive-stub':'HSTU /ACP1000-Ladder/ACP1000-TUBE:500',
          'matching-stub':'HSTU /YBS3/CIP00001:P,25',
          'native-matching-stub':'HSTU /YK-SS/1PK61:YKS-AAAAABEFN325'}
assert phase in commands
folder=e.OUT/('catalogue-'+phase+'-head-edit');folder.mkdir(exist_ok=False)
e.save(folder/'before.json',e.api('element/attributes',{'refno':'24384/26640'}))
e.tty(folder,['/CODEX_TYPES_CAT_BRAN',commands[phase],'Q ATT',"SAVEWORK 'Probe head "+phase+"'"])
print(folder)
