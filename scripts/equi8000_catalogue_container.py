"""Minimise the E3D catalogue fixture creation failure to its container step."""
import equi8000_live as e
folder=e.OUT/'catalogue-container-create';folder.mkdir(exist_ok=False)
assert not e.sql("SELECT VALUE id FROM pe WHERE name='/CODEX_TYPES_CAT_PIPE' AND deleted!=true;")[0]
e.tty(folder,['='+e.ROOT,'OWNER','Q REF','NEW PIPE /CODEX_TYPES_CAT_PIPE',
    'NEW BRAN /CODEX_TYPES_CAT_BRAN','Q REF','Q ATT',"SAVEWORK 'Catalogue route container'"])
print((folder/'tty.log').read_text(encoding='utf-8-sig'))
