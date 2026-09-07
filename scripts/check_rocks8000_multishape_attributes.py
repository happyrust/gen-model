"""Native fixture expectations checked against API and durable attribute rows."""
import sys
import equi8000_live as e

folder=e.OUT/sys.argv[1]
stage=sys.argv[2]
moved=stage in ('moved','moved-fixed')
direct=e.api('element/attributes', {'refno':'24384/26653'})
attrs={a['name']:a['value'] for a in direct['attributes']}
rows=e.sql('SELECT * FROM VALV:24384_26653;')[0]
assert len(rows)==1
expected={'NAME':'/CODEX_TYPES_MULTI_VALV','POS':[5000,1400 if stage=='moved-fixed' else (700 if moved else 0),0],
          'ORI':[0,0,0],'ORIL':True,'SHOP':moved,'BUIL':moved,'SPRE':'13245/643605'}
if stage == 'moved-fixed':
    expected['SHOP']=False
    expected['BUIL']=False
checks={}
for key,value in expected.items():
    durable_value='pe:'+value.replace('/','_') if key=='SPRE' else value
    checks[key]={'expected':value,'direct':attrs[key],'durable':rows[0][key],
                 'passed':attrs[key]==value and rows[0][key]==durable_value}
result={'passed':all(c['passed'] for c in checks.values()),'checks':checks}
e.save(folder/'valve-api-attributes.json',direct)
e.save(folder/'valve-durable-attributes.json',rows[0])
e.save(folder/'valve-attribute-comparison.json',result)
print(result)
assert result['passed']
