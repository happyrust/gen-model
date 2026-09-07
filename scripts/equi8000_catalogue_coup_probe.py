"""Use the SCOM's actual COUP type to isolate a design/catalogue type mismatch."""
import equi8000_live as e
folder=e.OUT/'catalogue-coup-create';folder.mkdir(exist_ok=False)
assert not e.sql("SELECT VALUE id FROM pe WHERE name='/CODEX_TYPES_COUP_BRAN' AND deleted!=true;")[0]
e.tty(folder,['/CODEX_TYPES_CAT_PIPE','NEW BRAN /CODEX_TYPES_COUP_BRAN',
    'HPOS E 0 N 0 U 0','TPOS E 120 N 0 U 0','HBOR 25','HCON OPEN','HSTU /YBS3/CIP00001:P,7',
    'NEW COUP /CODEX_TYPES_COUP_SREV','SPRE /DR07C/MACB200:25','POS E 120 N 0 U 0',
    'Q REF','Q ATT',"SAVEWORK 'Native COUP catalogue type control'"])
print((folder/'tty.log').read_text(encoding='utf-8-sig'))
