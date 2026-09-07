"""Repeat real edits, requiring automatic publication after every saved window."""
import json,struct,time,sys
import equi8000_live as e
base=e.OUT/(sys.argv[1] if len(sys.argv)>1 else 'panels-repeat');base.mkdir(exist_ok=False)
for index,width in enumerate([300,500,300,500,300,500]):
    folder=base/str(index);folder.mkdir()
    e.tty(folder,['/CODEX_TYPES_PANE_CUT',f'XLEN {width}',"SAVEWORK 'Panel repeated hole width'"])
    data=e.SOURCE.read_bytes()
    target=struct.unpack_from('>I',data,struct.unpack_from('>I',data,40)[0]*2048+12)[0]
    deadline=time.monotonic()+120;observations=[]
    while True:
        state=e.sql('SELECT published_target, publication_status FROM gen_root:24384_24776;')[0][0]
        queue=e.api('queue')
        observations.append(dict(state=state,queue=queue))
        if state.get('published_target',{}).get('source_end_sesno')==target:break
        if time.monotonic()>deadline:
            e.save(folder/'failed.json',dict(target=target,width=width,observations=observations))
            raise RuntimeError(f'window {target} did not publish automatically')
        time.sleep(2)
    e.save(folder/'passed.json',dict(target=target,width=width,observations=observations))
    print(f'PASS window={target} width={width}',flush=True)
