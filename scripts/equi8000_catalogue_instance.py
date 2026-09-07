"""Create a private route instance of a real catalogue geometry set."""
import equi8000_live as e
folder=e.OUT/'catalogue-instance-create';folder.mkdir(exist_ok=False)
assert not e.sql("SELECT VALUE id FROM pe WHERE name='/CODEX_TYPES_CAT_PIPE' AND deleted!=true;")[0]
e.tty(folder,['='+e.ROOT,'OWNER','NEW PIPE /CODEX_TYPES_CAT_PIPE',
    'NEW BRAN /CODEX_TYPES_CAT_BRAN','HPOS E 0 N 0 U 0','TPOS E 3000 N 0 U 0',
    'NEW VALV /CODEX_TYPES_CAT_SBOX','SPRE /SBCL150RFDD','POS E 0 N 0 U 0',
    'Q REF','Q ATT',"SAVEWORK 'Catalogue instance incremental fixture'"])
print((folder/'tty.log').read_text(encoding='utf-8-sig'))
