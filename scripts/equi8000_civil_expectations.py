"""Expected source data for civil live fixtures, independent of queried results."""
import sys
import equi8000_live as e
folder=e.OUT/sys.argv[1]
phase=sys.argv[2]
stage=phase
has_screed=phase.startswith('screed') or phase.startswith('holes')
if has_screed:phase='moved'
expected={}
for noun,x in [('FLOOR',0),('GWALL',2200)]:
    name='/CODEX_TYPES_'+noun+('_UPDATED' if phase=='moved' else '')
    expected[name]={'NAME':name,'POS':[x+(120 if phase=='moved' else 0),0,0]}
    expected['/CODEX_TYPES_'+noun+'_LOOP']={'HEIG':200 if phase=='created' else 350}
if has_screed:
    moved=stage=='screed-move' or stage.startswith('holes')
    name='/CODEX_TYPES_SCREED'+('_UPDATED' if moved else '')
    expected[name]={'NAME':name,'POS':[4520 if moved else 4400,0,0]}
    expected['/CODEX_TYPES_SCREED_LOOP']={'HEIG':200 if stage=='screed-created' else 350}
if stage.startswith('holes'):
    for noun in ['FLOOR','GWALL','SCREED']:
        expected['/CODEX_TYPES_'+noun+'_CUT']={'XLEN':500 if stage=='holes-resize' else 300,'YLEN':400,'ZLEN':1000,'POS':[0,0,175]}
    e.save(folder/'ray-probe-names.json',['/CODEX_TYPES_'+n+'_UPDATED' for n in ['FLOOR','GWALL','SCREED']])
e.save(folder/'expected-attributes.json',expected)
