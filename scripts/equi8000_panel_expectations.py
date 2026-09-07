"""Explicit PANE/PLOO data expectations for the saved live snapshots."""
import sys
import equi8000_live as e
folder=e.OUT/sys.argv[1]
phase=sys.argv[2]
updated=phase=='moved' or phase.startswith('hole')
name='/CODEX_TYPES_PANE'+('_UPDATED' if updated else '')
expected={name:{'NAME':name,'POS':[2320 if updated else 2200,0,0]},
    '/CODEX_TYPES_PANE_LOOP':{'HEIG':350 if phase in ['resized','moved'] or phase.startswith('hole') else 200}}
if phase.startswith('hole'):
    expected['/CODEX_TYPES_PANE_CUT']={'XLEN':500 if phase=='hole-resized' else 300,'YLEN':400,'ZLEN':1000,'POS':[0,0,175]}
    e.save(folder/'ray-probe-names.json',[name])
e.save(folder/'expected-attributes.json',expected)
