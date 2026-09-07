"""Repeat the orientation-only probe on the original short COUP control."""
import sys
import equi8000_live as e
phase=sys.argv[1] if len(sys.argv)>1 else 'set'
command={'set':'ORI Y IS N AND Z IS U','unset-fixed':'ORIFLAG FALSE'}[phase]
folder=e.OUT/('catalogue-coup-orientation-edit' if phase=='set' else 'catalogue-coup-orientation-'+phase+'-edit');folder.mkdir(exist_ok=False)
e.save(folder/'before.json',e.api('element/attributes',{'refno':'24384/26643'}))
e.tty(folder,['/CODEX_TYPES_COUP_SREV',command,'Q ATT',
             "SAVEWORK 'Explicit COUP orientation'"])
print(folder)
