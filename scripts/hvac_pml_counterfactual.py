"""Build an isolated diagnostic copy with only the new PML opcodes disabled."""
from pathlib import Path
import json
import re
import shutil
import subprocess
import hvac_sites_audit
import cpv_site_audit

root=Path(__file__).resolve().parents[1]
out=root/'output/hvac-flex-gril-fix-20260905/diagnostic-baseline'
out.mkdir(parents=True,exist_ok=True)
vendor=root.parent/'vendor'
for package in ['e3d-io','e3d-model']:
    source=vendor/package
    dest=out/package
    shutil.copytree(source/'src',dest/'src',dirs_exist_ok=True)
    if (source/'data').exists():
        shutil.copytree(source/'data',dest/'data',dirs_exist_ok=True)
    manifest=(source/'Cargo.toml').read_text(encoding='utf-8')
    def dependency(match):
        path=match.group(1)
        resolved=(out/'e3d-io') if package=='e3d-model' and path=='../e3d-io' else (source/path).resolve()
        return 'path = "'+resolved.as_posix()+'"'
    manifest=re.sub(r'path\s*=\s*"([^"]+)"',dependency,manifest)
    (dest/'Cargo.toml').write_text(manifest+'\n[workspace]\n',encoding='utf-8')
    shutil.copyfile(source/'Cargo.lock',dest/'Cargo.lock')
pml=out/'e3d-io/src/record/catalogue_pml.rs'
text=pml.read_text(encoding='utf-8')
start=text.index('fn parse(words:')
end=text.index('/// Render the expression',start)
parser=text[start:end]
for opcode in ['OP_POW','OP_MIN','OP_GREATER_THAN','OP_MATCH','OP_TRIM','OP_LOGICAL_STRING']:
    parser=parser.replace(' | '+opcode,'')
text=text[:start]+parser+text[end:]
pml.write_text(text,encoding='utf-8')
with (out/'build.log').open('w') as stream:
    subprocess.run(['cargo','rustc','--release','--manifest-path',str(out/'e3d-model/Cargo.toml'),
                    '--bin','gen_ams','--','-o',str(out/'gen_ams.exe')],
                   stdout=stream,stderr=subprocess.STDOUT,check=True)
target={'refno':'24381/46775'}
folder=out/'cam'; folder.mkdir(exist_ok=True)
hvac_sites_audit.TARGET=out
hvac_sites_audit.generate(target,folder)
report=cpv_site_audit.main(folder/'generated/elements.json',root/'output/hvac-sites-audit-20260905/24381_46775/native.json',folder/'audit.json')
print(json.dumps({k:v for k,v in report.items() if k!='results'}))
