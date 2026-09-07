"""Map remaining catalogue primitives to usable SCOM instances, read-only."""
import json,re,urllib.error
import equi8000_live as e
folder=e.OUT/'remaining-catalogue-map'; folder.mkdir(exist_ok=True)
cache={}
def attrs(ref):
    ref=ref.replace('_','/')
    if ref not in cache:
        try:
            cache[ref]=e.api('element/attributes',{'refno':ref})
        except urllib.error.HTTPError as error:
            e.save(folder/(ref.replace('/','_')+'-error.json'),{'refno':ref,'status':error.code,'body':error.read().decode()})
            raise
        e.save(folder/(ref.replace('/','_')+'.json'),cache[ref])
    raw=cache[ref]
    return raw,{a['name']:a['value'] for a in raw['attributes']}
results=[]; categories=set()
for sample in json.loads((e.OUT/'remaining-source-hierarchy/inventory.json').read_text()):
    if not sample['sample_ref']:continue
    ref=sample['sample_ref']; chain=[]; gmse=None
    for _ in range(10):
        try:raw,a=attrs(ref)
        except urllib.error.HTTPError:
            print('Failed attributes:',sample['noun'],ref,flush=True)
            break
        chain.append({'refno':ref,'noun':raw['noun']})
        if raw['noun']=='GMSE':gmse=ref
        if raw['noun']=='CATE':break
        ref=a['OWNER']
    else:raise ValueError(chain)
    if not chain or chain[-1]['noun']!='CATE':continue
    categories.add(ref)
    results.append({**sample,'chain':chain,'gmse':gmse,'category':ref})
commands=[]
for ref in sorted(categories):
    commands += ['$P CATEGORY-'+ref,'='+ref,'Q MEMB']
e.tty(folder,commands)
members={}
for chunk in (folder/'tty.log').read_text(encoding='utf-8-sig').split('CATEGORY-')[1:]:
    ref=chunk.splitlines()[0].strip()
    members[ref]=re.findall(r'^\s*\d+\s+SCOM\s+\d+\s*(/\S+|=\d+/\d+)',chunk,re.M)
for sample in results:
    candidates=[]
    for name in members[sample['category']]:
        # Resolve unnamed/named SCOMs from the direct name-search endpoint below
        if name.startswith('='): ref=name[1:]
        else:
            candidates.append({'name':name,'unresolved':True})
            continue
        raw,a=attrs(ref)
        if str(a.get('GMRE','')).replace('_','/')==sample['gmse']:
            candidates.append({'refno':raw['refno'],'name':a['NAME'],'gmref':a.get('GMRE'),'parameters':a.get('PARA')})
            break
    sample['candidates']=candidates
    e.save(folder/'result.json',results)
    print(sample['noun'],sample['gmse'],candidates[:2],flush=True)
