import json
import urllib.error
import equi8000_live as e
rows=json.loads((e.OUT/'profiles-final-moved'/'all-attributes.json').read_text())
try: print(json.dumps(e.api('element/attributes',{'refno':rows[0]['refno']}),ensure_ascii=False))
except urllib.error.HTTPError as ex: print(ex.code,ex.read().decode())
