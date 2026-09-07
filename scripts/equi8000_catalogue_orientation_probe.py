"""Explicitly orient the original valve without changing its identity rotation."""
import sys
import equi8000_live as e
phase=sys.argv[1] if len(sys.argv)>1 else 'set'
command={'set':'ORI Y IS N AND Z IS U','unset':'ORIFLAG FALSE','set-fixed':'ORI Y IS N AND Z IS U','unset-fixed':'ORIFLAG FALSE'}[phase]
folder=e.OUT/('catalogue-orientation-'+phase+'-edit');folder.mkdir(exist_ok=False)
e.save(folder/'before.json',e.api('element/attributes',{'refno':'24384/26641'}))
e.tty(folder,['/CODEX_TYPES_CAT_SREV_UPDATED',command,'Q ATT',
             "SAVEWORK 'Explicit catalogue valve orientation'"])
print(folder)
