"""Regenerate the HVAC scopes against the pinned native oracle from the audit."""
from pathlib import Path
import argparse
import json
import shutil
import hvac_sites_audit
import cpv_site_audit

root = Path(__file__).resolve().parents[1]
baseline = root / 'output/hvac-sites-audit-20260905'
out = root / 'output/hvac-flex-gril-fix-20260905'
parser = argparse.ArgumentParser()
parser.add_argument('--only', nargs='+')
args = parser.parse_args()
summary = []
for target in json.loads((baseline/'targets.json').read_text(encoding='utf-8')):
    if args.only and target['refno'] not in args.only:
        continue
    folder = out / target['refno'].replace('/', '_')
    folder.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(baseline/folder.name/'native.json',folder/'native.json')
    hvac_sites_audit.generate(target, folder)
    report = cpv_site_audit.main(folder/'generated/elements.json',folder/'native.json',folder/'audit.json')
    summary.append({**target, **{k:v for k,v in report.items() if k!='results'}})
(out/'summary.json').write_text(json.dumps(summary,ensure_ascii=False,indent=2),encoding='utf-8')
