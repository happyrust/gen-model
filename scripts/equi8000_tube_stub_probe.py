"""Read a real tube stub selection for the missing-stub differential probe."""
import equi8000_live as e
folder=e.OUT/'catalogue-tube-stub-probe';folder.mkdir(exist_ok=False)
e.tty(folder,['VAR !items COLLECT ALL BRAN WITH ( HSTU NE NULREF )',
    'DO !item VALUES !items',' $!item',' Q REF',' Q HSTU',' Q HBOR',' BREAK','ENDDO'])
print((folder/'tty.log').read_text(encoding='utf-8-sig'))
