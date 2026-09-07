"""One-variable native head-tube probes on the isolated positive clone."""
import sys
import equi8000_live as e
phase=sys.argv[1];folder=e.OUT/('positive-head-edit-'+phase);folder.mkdir(exist_ok=False)
changes={'no-pspec':'PSPE NULREF','no-href':'HREF NULREF','no-hcon':'HCON UNSET',
         'no-hstub':'HSTU NULREF','restore-pspec':'PSPE /ACP1000-Ladder',
         'open-hcon':'HCON OPEN',
         'restore-hstub':'HSTU /ACP1000-Ladder/ACP1000-TUBE:500',
         'clear-hcon-fixed':'HCON UNSET',
         'head-bore25':'HBOR 25',
         'restore-head-bore500':'HBOR 500',
         'stub-nominal25':'HSTU /YK-SS/1PK61:YKS-AAAAABEFN325',
         'head-bore25-fixed':'HBOR 25',
         'head-bore500-fixed':'HBOR 500',
         'restore-href':'HREF /LV-CO-1RX-3062D',
         'connected-no-stub':'HSTU NULREF',
         'stub-restored-fixed':'HSTU /YK-SS/1PK61:YKS-AAAAABEFN325',
         'stub-cleared-fixed':'HSTU NULREF'}
assert phase in changes
e.save(folder/'before.json',e.api('element/attributes',{'refno':'24384/26644'}))
e.tty(folder,['/CODEX_TYPES_POS_HEAD',changes[phase],'Q ATT',"SAVEWORK 'Positive head "+phase+"'"])
print(str(folder))
