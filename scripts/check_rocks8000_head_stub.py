"""Assert a native head-stub increment in both the API and durable BRAN data."""
import sys
import equi8000_live as e

folder = e.OUT / sys.argv[1]
expected_stub = sys.argv[2]
direct = e.api('element/attributes', {'refno': '24384/26644'})
attrs = {a['name']: a for a in direct['attributes']}
rows = e.sql('SELECT * FROM BRAN:24384_26644;')[0]
assert len(rows) == 1
durable = rows[0]
expected = {'HSTU': expected_stub, 'HREF': '24384/3181', 'PSPE': '0/0', 'HBOR': 500, 'HCON': ''}
checks = {}
for name, value in expected.items():
    stored = 'pe:' + value.replace('/', '_') if name in ('HSTU', 'HREF', 'PSPE') else value
    checks[name] = {'expected': value, 'direct': attrs[name]['value'], 'durable': durable[name],
                    'passed': attrs[name]['value'] == value and durable[name] == stored}
result = {'passed': all(c['passed'] for c in checks.values()), 'checks': checks}
e.save(folder / 'stub-direct-attributes.json', direct)
e.save(folder / 'stub-durable-attributes.json', durable)
e.save(folder / 'stub-attribute-comparison.json', result)
print(result)
assert result['passed']
