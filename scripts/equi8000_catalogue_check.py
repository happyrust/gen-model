"""Verify catalogue selection, scalar data and independently exported geometry."""
import json,subprocess,sys
import equi8000_live as e
phase=sys.argv[1]
assert phase in ('created','resized','moved','moved-aligned')
folder=e.OUT/('catalogue-native-'+phase)
assert not (folder/'publication-timeout.json').exists()
moved=phase.startswith('moved')
name='/CODEX_TYPES_CAT_SREV_UPDATED' if moved else '/CODEX_TYPES_CAT_SBOX'
e.save(folder/'expected-attributes.json',{name:{'NAME':name,'POS':[120 if moved else 0,0,0],
                                               'SHOP':moved,'BUIL':moved}})
subprocess.run([sys.executable,str(e.REPO/'scripts/compare_equi8000_types.py'),folder.name,'VALV'],check=True,stdout=subprocess.DEVNULL)
result=json.loads((folder/'types-comparison.json').read_text())
assert result['passed'],result
entry=next(a for a in json.loads((folder/'all-attributes.json').read_text()) if a['refno']=='24384/26641')
direct={a['name']:a['value'] for a in entry['direct']['attributes']}
expected='15192/245830' if phase=='created' else '15192/245842'
reference={'expected':expected,'direct':direct['SPRE'],'durable':entry['durable'][0]['SPRE']}
reference['passed']=reference['direct']==expected and reference['durable']=='pe:'+expected.replace('/','_')
result['selection_reference']=reference
result['passed'] &= reference['passed']
if phase!='created':
    baseline=json.loads((e.OUT/'catalogue-native-created/types-comparison.json').read_text())['comparisons'][0]
    current=result['comparisons'][0]
    result['shape_changed']=current['mesh']!=baseline['mesh'] and abs(current['plant_volume']-baseline['plant_volume'])>1
    result['passed'] &= result['shape_changed']
e.save(folder/'catalogue-comparison.json',result)
assert result['passed'],result
print(json.dumps(result,ensure_ascii=False))
