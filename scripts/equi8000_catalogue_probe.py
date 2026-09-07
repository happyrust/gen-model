"""Read catalogue attributes and database identity without changing source data."""
import json
import equi8000_live as e
folder=e.OUT/'remaining-catalogue-detail'; folder.mkdir(exist_ok=False)
inventory=json.loads((e.OUT/'remaining-source-hierarchy/inventory.json').read_text())
commands=[]
for row in inventory:
    if row['sample_ref']:
        commands += ['$P DETAIL-'+row['noun'], '='+row['sample_ref'],
                     'Q REF', 'Q ATT', 'Q DBNAME', 'Q DBFILE']
commands += ['$P GMSE-REFERENCE', '=15192/255280', 'Q ATT', 'OWNER', 'Q ATT', 'Q MEMB']
e.tty(folder, commands)
print(str(folder))
