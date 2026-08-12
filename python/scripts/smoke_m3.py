# -*- coding: utf-8 -*-
"""M3 验收冒烟：执行层（full_init + 增量 apply + 单根生成 + 房间 drain）。

会真实写库（AMS 测试工程）：model.ensure 会为目标 EQUI 生成模型实例。
前提：gen-model 服务已停止（full_init 要拿同一把单实例锁）。
积压的 195 条 pending 不在这里消化（incr.drain_data 会全量跑生成，跑不跑由人决定）。

**历史验收记录，不可原样复跑**（2026-08-12 起）：本脚本钉在 M3 当时的环境上
——仓库根 `DbOption` + 8009 正式库 + 真实 AMS 工程目录。8009 的数据目录已被
SurrealDB 3.x 写坏且决定不修（见 `python/testbed/README.md`），照原样跑必失败；
何况它会真实写库，别拿它去试新环境。
等价物：真数据全链路用 `python/testbed/run_full_loop.py`（8019 沙箱 + 项目副本，
不用停任何服务）；执行层的行为断言用 `pytest -m "not offline"`（一次性内存库）。
"""

import json
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

REPO = Path(__file__).resolve().parents[2]

import aios_db

DB_FILE = r"D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams7997_0001"
TARGET = "24381_100677"  # /-RX-CUP-001FA（EQUI，冒烟前 inst_relate 为 0）


def main() -> None:
    print("== 硬守护：full_init 之前 mutating 必须报错 ==")
    try:
        aios_db.model.ensure(TARGET)
        raise AssertionError("守护失效：未 full_init 竟然放行了")
    except RuntimeError as error:
        print(f"  如期拒绝: {str(error)[:60]}...")

    print("== full_init（拿单实例锁 + run_cli 前置段）==")
    aios_db.set_config(str(REPO / "DbOption"))
    aios_db.full_init(cwd=str(REPO))
    print("  full_init 完成")

    print("== incr.apply_file（rollback 现场应判 up_to_date）==")
    result = aios_db.incr.apply_file(DB_FILE)
    print(f"  {json.dumps(result, ensure_ascii=False)}")
    assert result["up_to_date"], "水位 238 > 文件 102，应该 up_to_date"

    print("== model.ensure：幂等复用 + force 重生成 ==")
    outcome = aios_db.model.ensure(TARGET)
    print(f"  首次 ensure: status={outcome['status']} "
          f"model_instances={outcome.get('model_instance_count')} "
          f"generated={outcome.get('generated_instance_count')}")
    assert outcome["status"] == "AlreadyAvailable", "已有实例应直接复用，不再跑生成"

    forced = aios_db.model.ensure(TARGET, force=True)
    print(f"  force 重生成: status={forced['status']} "
          f"model_instances={forced.get('model_instance_count')} "
          f"generated={forced.get('generated_instance_count')}")
    assert forced["status"] == "Generated", "force 应真的重跑生成"

    insts = aios_db.db.inst(TARGET)
    print(f"  子树 inst_relate: {len(insts)} 条")
    assert len(insts) == forced.get("generated_instance_count"), "实例边数应与生成数一致"
    with_aabb = [row for row in insts if row.get("aabb")]
    if with_aabb:
        print(f"    含 aabb {len(with_aabb)} 条，首条: "
              f"{json.dumps(with_aabb[0]['aabb'], ensure_ascii=False)[:160]}")

    print("== room.drain（消化待重算房间目标）==")
    report = aios_db.room.drain()
    print(f"  {json.dumps(report, ensure_ascii=False)}")

    print("== 积压现状（不在冒烟里消化）==")
    units = aios_db.db.pending_model_units()
    print(f"  pending 模型单元仍有 {len(units)} 行；要消化请自行运行 aios_db.incr.drain_data()")

    print("\nM3 冒烟全部通过。")


if __name__ == "__main__":
    main()
