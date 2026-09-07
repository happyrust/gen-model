"""Create an isolated clone of a proven native head-tube branch."""
import equi8000_live as e
folder=e.OUT/'catalogue-positive-clone-create';folder.mkdir(exist_ok=False)
assert not e.sql("SELECT VALUE id FROM pe WHERE name='/CODEX_TYPES_POS_HEAD' AND deleted!=true;")[0]
e.tty(folder,['/CODEX_TYPES_CAT_PIPE','NEW BRAN /CODEX_TYPES_POS_HEAD COPY /LV-CO-1RX-3062E',
    'Q REF','Q MEMB',"SAVEWORK 'Positive native tube control copy'"])
print((folder/'tty.log').read_text(encoding='utf-8-sig'))
