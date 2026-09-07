"""Equipment primitive/negative-family live incremental matrix in isolated AMS."""
import json,sys
import equi8000_live as e
P='/CODEX_TYPES_'
SPECS={
 'CYLI':({'DIAM':400,'HEIG':600},'DIAM',520,'NCYL'),
 'CONE':({'DBOT':500,'DTOP':200,'HEIG':600},'DTOP',320,'NCON'),
 'PYRA':({'XBOT':500,'YBOT':400,'XTOP':200,'YTOP':160,'HEIG':600},'XTOP',300,'NPYR'),
 'RTOR':({'RINS':250,'ROUT':450,'HEIG':200,'ANGL':180},'ANGL',240,'NRTO'),
 'CTOR':({'RINS':250,'ROUT':450,'ANGL':180},'ANGL',240,'NCTO'),
 'SNOU':({'DBOT':500,'DTOP':200,'HEIG':600,'XOFF':80,'YOFF':40},'XOFF',140,'NSNO'),
 'DISH':({'DIAM':600,'HEIG':180,'RADI':0},'HEIG',240,'NDIS'),
 'SLCY':({'DIAM':400,'HEIG':600,'XTSH':10,'YTSH':5,'XBSH':-5,'YBSH':0},'XTSH',20,'NSLC'),
}
def specs():
    rows=[]
    for noun,(attrs,attr,value,neg) in SPECS.items():
        rows.extend([dict(noun=noun,attrs=attrs,attr=attr,value=value,negative=False),dict(noun=neg,attrs=attrs,attr=attr,value=value,negative=True)])
    return rows
def run(phase,only=None):
    folder=e.OUT/('types-'+phase+(('-'+only) if only else (('-'+sys.argv[2]) if len(sys.argv)>2 else ''))+(('-'+sys.argv[3]) if len(sys.argv)>3 else ''));folder.mkdir(exist_ok=False)
    commands=[]
    for i,s in enumerate(specs()[:1] if phase=='single' else specs()):
        if only and s['noun']!=only:continue
        if phase=='radial' and s['noun'] not in ['RTOR','NRTO','CTOR','NCTO']:continue
        name=P+s['noun'];host=P+'HOST_'+s['noun'];x=(i%4)*2200;y=(i//4)*2200
        if phase=='create':
            commands+=['='+e.ROOT]
            if s['negative']:
                commands+=['NEW BOX '+host,'XLEN 1200','YLEN 1200','ZLEN 800',f'POS E {x} N {y} U 0']
            commands+=['NEW '+s['noun']+' '+name]+[f'{k} {v}' for k,v in s['attrs'].items()]
            if not s['negative']: commands += [f'POS E {x} N {y} U 0']
        elif phase in ('resize','single'):commands += [name,f"{s['attr']} {s['value']}"]
        elif phase=='radial':commands += [name+'_UPDATED','ROUT 520']
        elif phase=='move':commands += [name,'BY E 120','NAME '+name+'_UPDATED']
        elif phase=='restore':commands += [host if s['negative'] else name+'_UPDATED','DELETE '+('BOX' if s['negative'] else s['noun'])]
        commands+=['Q CE','Q REF']
    commands += ["SAVEWORK 'Equipment types "+phase+"'"]
    e.tty(folder,commands)
    e.save(folder/'specs.json',specs());print(folder)
if __name__=='__main__':
    if sys.argv[1].endswith('-split'):
        for spec in specs():
            try:run(sys.argv[1].removesuffix('-split'),spec['noun'])
            except Exception as ex:print(spec['noun'],str(ex),flush=True)
    else:run(sys.argv[1],sys.argv[2] if len(sys.argv)>2 and sys.argv[2] in [s['noun'] for s in specs()] else None)
