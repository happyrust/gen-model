"""Map live AMS 8000 design SPRE values to visible catalogue member nouns."""
from collections import Counter, defaultdict
import re
import sys

import equi8000_live as e


TABLES = ('ATTA', 'BEND', 'COUP', 'ELBO', 'FITT', 'REDU', 'VALV', 'WELD')
suffix = '-' + sys.argv[1] if len(sys.argv) > 1 else ''
ROOT = e.OUT / ('remaining-live-spco-inventory' + suffix)
ROOT.mkdir(exist_ok=False)
scope = sys.argv[1] if len(sys.argv) > 1 else ''
if scope == 'duct':
    ALLOWED_DBNUMS = {'13244'}
elif scope.startswith('db') and scope[2:].isdigit():
    ALLOWED_DBNUMS = {scope[2:]}
else:
    ALLOWED_DBNUMS = None
# These refs crash the native batch query at Q NAME/Q CATR. Each failed batch is
# preserved; 13245/643605 is already mapped by the focused VALV probe.
SKIP_REFS = {'13244/272462', '13245/643605'}


def native(stage, commands):
    folder = ROOT / stage
    folder.mkdir()
    e.tty(folder, commands)
    return (folder / 'tty.log').read_text(encoding='utf-8-sig')


queries = ''.join(
    f'SELECT VALUE SPRE FROM {table} WHERE SPRE != NONE LIMIT 10000;'
    for table in TABLES
)
by_ref = defaultdict(set)
for table, values in zip(TABLES, e.sql(queries)):
    for value in values:
        match = re.fullmatch(r'pe:(\d+)_(\d+)', str(value))
        if match and match.groups() != ('0', '0'):
            ref = '/'.join(match.groups())
            if ref not in SKIP_REFS and (ALLOWED_DBNUMS is None or match.group(1) in ALLOWED_DBNUMS):
                by_ref[ref].add(table)

commands = []
for ref in sorted(by_ref):
    commands += [f'$P SPCO-{ref}', f'={ref}', 'Q REF', 'Q NAME', 'Q CATR']
spco_log = native('spco', commands)

spcos = {}
cat_tokens = set()
for chunk in spco_log.split('SPCO-')[1:]:
    ref = chunk.splitlines()[0].strip()
    name = re.search(r'^Name\s+(.+)$', chunk, re.MULTILINE)
    cat = re.search(r'^Catref\s+(.+)$', chunk, re.MULTILINE)
    if not cat:
        continue
    token = cat.group(1).strip()
    cat_tokens.add(token)
    spcos[ref] = {
        'tables': sorted(by_ref[ref]),
        'name': name.group(1).strip() if name else '',
        'catref': token,
    }

commands = []
for token in sorted(cat_tokens):
    commands += [f'$P SCOM-{token}', token, 'Q REF', 'Q GTYP', 'Q GMRE', 'Q PARA']
scom_log = native('scom', commands)

scoms = {}
gm_tokens = set()
for chunk in scom_log.split('SCOM-')[1:]:
    token = chunk.splitlines()[0].strip()
    ref = re.search(r'^Ref\s*=(\d+/\d+)$', chunk, re.MULTILINE)
    gtyp = re.search(r'^Gtype\s+(.+)$', chunk, re.MULTILINE)
    gmref = re.search(r'^Gmref\s+(.+)$', chunk, re.MULTILINE)
    if not gmref:
        continue
    gm_token = gmref.group(1).strip()
    gm_tokens.add(gm_token)
    scoms[token] = {
        'refno': ref.group(1) if ref else '',
        'gtyp': gtyp.group(1).strip() if gtyp else '',
        'gmref': gm_token,
    }

commands = []
for token in sorted(gm_tokens):
    commands += [f'$P GMSET-{token}', token, 'Q REF', 'Q MEMB']
gmset_log = native('gmset', commands)

gmsets = {}
for chunk in gmset_log.split('GMSET-')[1:]:
    token = chunk.splitlines()[0].strip()
    ref = re.search(r'^Ref\s*=(\d+/\d+)$', chunk, re.MULTILINE)
    members = re.findall(r'^\s*\d+\s+([A-Z0-9]+)\s+', chunk, re.MULTILINE)
    gmsets[token] = {
        'refno': ref.group(1) if ref else '',
        'member_nouns': dict(sorted(Counter(members).items())),
    }

for item in spcos.values():
    item['scom'] = scoms.get(item['catref'])
    if item['scom']:
        item['gmset'] = gmsets.get(item['scom']['gmref'])

result = {
    'tables': list(TABLES),
    'spco_count': len(spcos),
    'spcos': spcos,
}
e.save(ROOT / 'inventory.json', result)
print('SPCO', len(spcos), 'SCOM', len(scoms), 'GMSET', len(gmsets))
for ref, item in spcos.items():
    members = (item.get('gmset') or {}).get('member_nouns')
    if members:
        print(ref, ','.join(item['tables']), item['scom']['gtyp'], members)
