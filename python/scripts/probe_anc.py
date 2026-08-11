# -*- coding: utf-8 -*-
import json
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

import aios_db

REPO = Path(__file__).resolve().parents[2]
aios_db.set_config(str(REPO / "DbOption"))
aios_db.connect(cwd=str(REPO))

rows = aios_db.db.query(
    "SELECT in, anc FROM inst_relate WHERE in.dbnum = 7997 LIMIT 2;"
)
print(json.dumps(rows, ensure_ascii=False, indent=1)[:700])

u64 = (24381 << 32) | 100677
rows = aios_db.db.query(
    "SELECT count() FROM inst_relate WHERE anc CONTAINS $u GROUP ALL;", {"u": u64}
)
print("u64 hit:", json.dumps(rows, ensure_ascii=False))
