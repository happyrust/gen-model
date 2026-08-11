# -*- coding: utf-8 -*-
"""M2 验收冒烟：连接层观察面 + HTTP 客户端。

前提：SurrealDB fork 服务在跑（DbOption.toml 的 v_ip:v_port）。
gen-model Web 服务（8022）不在跑时，HTTP 客户端部分降级提示。
"""

import json
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "python"))

import aios_db
from aios_client import AiosApiError, AiosClient

DBNUM = 7997


def main() -> None:
    aios_db.set_config(str(REPO / "DbOption"))
    aios_db.connect(cwd=str(REPO))

    print("== db.watermark ==")
    watermark = aios_db.db.watermark(DBNUM)
    print(f"  dbnum {DBNUM} 应用水位: {watermark}")
    assert watermark > 0

    print("== db.query 抓一个有名元素 ==")
    rows = aios_db.db.query(
        "SELECT record::id(id) AS refno, name FROM pe \
         WHERE dbnum = $dbnum AND name != NONE AND deleted = false AND noun = 'EQUI' LIMIT 1;",
        {"dbnum": DBNUM},
    )[0]
    assert rows, "dbnum 里应有带名 EQUI"
    refno, name = rows[0]["refno"], rows[0]["name"]
    print(f"  {name} -> {refno}")

    print("== db.by_name 往返 ==")
    hits = aios_db.db.by_name(name, dbnum=DBNUM)
    print(f"  by_name({name!r}) -> {hits}")
    assert refno in hits

    print("== db.pe / members / owner_chain / inst ==")
    pe_row = aios_db.db.pe(refno)
    print(f"  pe.noun={pe_row['noun']} sesno={pe_row.get('sesno')}")
    members = aios_db.db.members(refno)
    print(f"  直接成员 {len(members)} 个: {[m['noun'] for m in members][:8]}")
    chain = aios_db.db.owner_chain(refno)
    print("  owner 链: " + " <- ".join(f"{n['noun']}" for n in chain))
    assert chain[-1]["noun"].upper() in ("WORL", "WORLD")
    insts = aios_db.db.inst(refno)
    print(f"  inst_relate 边 {len(insts)} 条")
    if insts:
        aabb = insts[0].get("aabb")
        print(f"    首条 aabb: {json.dumps(aabb, ensure_ascii=False)[:160]}")

    print("== db.pending_model_units / window_blocks / root_attempts ==")
    units = aios_db.db.pending_model_units()
    print(f"  待重试模型单元 {len(units)} 行（死信 {sum(1 for u in units if u['dead'])} 行）")
    blocks = aios_db.db.window_blocks()
    print(f"  窗口阻断 {len(blocks)} 行")
    attempts = aios_db.db.root_attempts(DBNUM)
    print(f"  dbnum {DBNUM} 根失败记录 {len(attempts)} 条")

    print("== db.dbnum_statuses（登记表 ∪ 项目扫描）==")
    report = aios_db.db.dbnum_statuses()
    rows = report["dbnums"]
    print(f"  共 {len(rows)} 行；示例:")
    for row in rows[:4]:
        print(
            f"    dbnum={row['dbnum']} {row.get('db_type')} applied={row.get('applied_sesno')} "
            f"file_latest={row.get('file_latest_sesno')} blocked={row.get('blocked')}"
        )

    print("== db.preview_manual_update（只读预览）==")
    preview = aios_db.db.preview_manual_update()
    print(
        f"  up_to_date={preview['up_to_date']} dbnums={len(preview['dbnums'])} "
        f"pending_retries={len(preview.get('pending_model_retries', []))} "
        f"warnings={len(preview.get('warnings', []))}"
    )
    for row in preview["dbnums"][:4]:
        print(
            f"    dbnum={row['dbnum']} applied={row['applied_sesno']} "
            f"file_latest={row['file_latest_sesno']} 净变化 +{row['net_added']}/~{row['net_modified']}/-{row['net_deleted']}"
        )

    print("== aios_client（Web 服务未跑则降级提示）==")
    client = AiosClient()
    try:
        health = client.health()
        print(f"  /health: {health}")
        dbnums = client.dbnums()
        print(f"  /dbnums: {len(dbnums['dbnums'])} 行")
        queue = client.queue()
        print(f"  /queue: paused={queue['paused']} rows={len(queue['rows'])}")
    except (OSError, AiosApiError) as error:
        print(f"  [降级] Web 服务不可用（8022 未监听？）: {error}")

    print("\nM2 冒烟全部通过。")


if __name__ == "__main__":
    main()
