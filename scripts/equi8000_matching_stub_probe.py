"""Discover an existing native head stub used on a bore-25 branch."""
import equi8000_live as e
folder=e.OUT/'catalogue-matching-stub-discovery';folder.mkdir(exist_ok=False)
e.tty(folder,['VAR !items COLLECT ALL BRAN WITH ( HSTU NE NULREF AND HBOR EQ 25 )',
             'DO !item VALUES !items',' $!item',' Q REF',' Q HSTU',' Q HBOR',' BREAK','ENDDO'])
print((folder/'tty.log').read_text(encoding='utf-8-sig'))
