"""Live regression for native HCON UNSET on the isolated positive BRAN."""
import json, sys
import equi8000_live as e

attribute = next(a for a in e.api('element/attributes', {'refno':'24384/26644'})['attributes'] if a['name']=='HCON')
durable = e.sql('SELECT HCON FROM BRAN:24384_26644;')[0][0]['HCON']
result = {'attribute':attribute, 'durable':durable,
          'passed':attribute['value']==durable=='' and attribute['is_unset'] and attribute['display']=='unset'}
if len(sys.argv)>1:
    e.save(e.OUT/sys.argv[1], result)
print(json.dumps(result))
assert result['passed'], 'Native HCON UNSET must not render as the WORD 0'
