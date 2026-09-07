"""Live profile and polyhedral equipment fixtures, isolated AMS only."""
import sys
import equi8000_live as e
P='/CODEX_TYPES_'
NOUNS=['EXTR','NXTR','REVO','NREV','POLYHE','NPOLYH','POHE']
def run(noun,phase):
    retry='legacy' in sys.argv[2:]
    suffix='-'.join(sys.argv[2:])
    folder=e.OUT/(f'profiles-{phase}-{noun}'+('-'+suffix if suffix else ''));folder.mkdir(exist_ok=False)
    name=P+noun;host=P+'HOST_'+noun;neg=noun.startswith('N');cmd=[]
    if phase=='recreate':
        cmd=[host if neg else name,'DELETE '+('BOX' if neg else noun)]
        phase='create'
    if phase=='create':
        cmd+=['='+e.ROOT]
        if neg:cmd+=['NEW BOX '+host,'XLEN 1600','YLEN 1600','ZLEN 1200',f'POS E {NOUNS.index(noun)*2200} N 11000 U 0']
        cmd+=['NEW '+noun+' '+name]
        if not neg:cmd += [f'POS E {NOUNS.index(noun)*2200} N 11000 U 0']
        if noun in ['EXTR','NXTR','REVO','NREV']:
            if 'REV' in noun:cmd+=['ANGL 270']
            cmd+=(['HEIG 500','NEW LOOP '+name+'_LOOP'] if retry and 'REV' not in noun else (['NEW LOOP '+name+'_LOOP'] if retry else ['NEW PLOO '+name+'_LOOP','HEIG 500']))
            points=[(0,200),(500,200),(500,500),(0,500)] if 'REV' in noun else [(-300,-300),(300,-300),(300,300),(-300,300)]
            for i,(x,y) in enumerate(points):cmd+=['NEW '+('VERT' if retry else 'PAVE')+' '+name+'_V'+str(i),f'POS E {x} N {y} U 0']
        else:
            pts=[(-300,-300,-300),(300,-300,-300),(300,300,-300),(-300,300,-300),(-300,-300,300),(300,-300,300),(300,300,300),(-300,300,300)]
            faces=[[0,3,2,1],[4,5,6,7],[0,1,5,4],[1,2,6,5],[2,3,7,6],[3,0,4,7]]
            if noun!='POHE':
                cmd+=['NEW POLPTL']
                for i,(x,y,z) in enumerate(pts):cmd+=['NEW POIN '+name+'_V'+str(i),f'POS E {x} N {y} U {z}']
            for face in faces:
                cmd += [name]
                if 'unordered' not in sys.argv and (noun!='POHE' or face!=faces[0]):cmd += ['LAST MEM']
                cmd += ['NEW '+('POGO' if noun=='POHE' else 'POLFAC')]
                if noun=='POHE':
                    for i in face:
                        x,y,z=pts[i];cmd+=['NEW POIN',f'POS E {x} N {y} U {z}']
                else:cmd+=['NEW POLOOP','NEW LOOPTS','VXREF '+' '.join(name+'_V'+str(i) for i in face)]
    elif phase=='resize':
        if noun in ['EXTR','NXTR']:cmd=[name,'HEIG 700']
        elif 'REV' in noun:cmd=[name,'ANGL 180']
        elif noun!='POHE':
            for i,y,z in [(1,-300,-300),(2,300,-300),(5,-300,300),(6,300,300)]:cmd += [name+'_V'+str(i),f'POS E 400 N {y} U {z}']
        else:
            pts=[(-300,-300,-300),(300,-300,-300),(300,300,-300),(-300,300,-300),(-300,-300,300),(300,-300,300),(300,300,300),(-300,300,300)]
            faces=[[0,3,2,1],[4,5,6,7],[0,1,5,4],[1,2,6,5],[2,3,7,6],[3,0,4,7]]
            for fi,face in enumerate(faces):
                for vi,index in enumerate(face):
                    x,y,z=pts[index]
                    if x!=300:continue
                    cmd += [name,'FIRST MEM']+['NEXT MEM']*fi+['FIRST MEM']+['NEXT MEM']*vi
                    cmd += [f'POS E 400 N {y} U {z}',f'NAME {name}_F{fi}_V{vi}']
    elif phase=='move':cmd=[name,'BY E 120','NAME '+name+'_UPDATED']
    elif phase=='restore':cmd=[host if neg else name+'_UPDATED','DELETE '+('BOX' if neg else noun)]
    e.tty(folder,cmd+['Q CE','Q REF',"SAVEWORK 'Equipment profile "+phase+"'"])
    print(noun,phase,folder,flush=True)
if __name__=='__main__':
    selected=([sys.argv[2]] if len(sys.argv)>2 and sys.argv[2] in NOUNS else (NOUNS[:4] if len(sys.argv)>2 and sys.argv[2]=='legacy' else NOUNS))
    for noun in selected:
        try:run(noun,sys.argv[1])
        except Exception as ex:print(noun,ex,flush=True)
