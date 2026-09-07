"""Read native selections for the demonstrated catalogue geometry set."""
import equi8000_live as e
folder=e.OUT/'catalogue-native-sizes';folder.mkdir(exist_ok=False)
e.tty(folder,['VAR !items COLLECT ALL SPCO WITH ( GMRE OF CATR EQ =15192/225911 )',
    'DO !item VALUES !items',' $!item',' Q REF',' Q NAME',' Q CATR','ENDDO'])
print((folder/'tty.log').read_text(encoding='utf-8-sig'))
