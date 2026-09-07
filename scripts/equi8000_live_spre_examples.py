"""Print live design elements for selected catalogue SPCO references."""
import json
import sys

import equi8000_live as e


TABLES = ('ATTA', 'BEND', 'COUP', 'ELBO', 'FITT', 'REDU', 'VALV', 'WELD')
for ref in sys.argv[1:]:
    record = 'pe:' + ref.replace('/', '_')
    queries = ''.join(
        f'SELECT id,name,OWNER,SPRE,POS,ORI FROM {table} WHERE SPRE={record} LIMIT 5;'
        for table in TABLES
    )
    rows = {
        table: values for table, values in zip(TABLES, e.sql(queries)) if values
    }
    owners = sorted({row['OWNER'] for values in rows.values() for row in values})
    owner_rows = []
    for owner in owners:
        owner_rows.extend(e.sql(f"SELECT id,name,OWNER FROM BRAN:{owner.removeprefix('pe:')};")[0])
    print(ref, json.dumps({'elements': rows, 'owners': owner_rows}, ensure_ascii=False))
