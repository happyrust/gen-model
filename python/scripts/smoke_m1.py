# -*- coding: utf-8 -*-
"""M1 验收冒烟：解析层四个函数 + 连接层 query。

用法（在 python/ 目录）:
    .venv\\Scripts\\python scripts\\smoke_m1.py [库文件路径]

库文件缺省取 AMS 样例工程的 ams7997_0001。连接层验收需要 SurrealDB fork
服务在跑（DbOption.toml 的 v_ip:v_port）；连不上时只降级提示，不算失败——
M1 的硬验收是解析层。
"""

import json
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

import aios_db

DEFAULT_DB = r"D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams7997_0001"


def main() -> None:
    db_file = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_DB
    assert Path(db_file).exists(), f"库文件不存在: {db_file}"

    # 解析深处也会读全局 DbOption（debug 选项等），必须最先指定配置。
    aios_db.set_config(str(Path(__file__).resolve().parents[2] / "DbOption"))

    print("== parse.is_db_file ==")
    ok = aios_db.parse.is_db_file(db_file)
    print(f"  {db_file} -> {ok}")
    assert ok, "候选库文件判定应为 True"

    print("== parse.header ==")
    header = aios_db.parse.header(db_file)
    print(json.dumps(header, ensure_ascii=False, indent=2))
    assert header["dbnum"] > 0 and header["latest_sesno"] > 0

    print("== parse.sessions ==")
    sessions = aios_db.parse.sessions(db_file)
    print(f"  共 {len(sessions)} 个会话页，最早 {sessions[0]['sesno']}，最新 {sessions[-1]['sesno']}")
    for row in sessions[-3:]:
        print(f"  sesno={row['sesno']} date={row['date']} user={row['computer_name']!r}")
    # 一致性：文件头的最新会话号 == 会话页里最大的会话号
    assert sessions[-1]["sesno"] == header["latest_sesno"], "header 与会话页的最新会话号不一致"

    print("== parse.collect_changes ==")
    start = max(2, header["latest_sesno"] - 2)
    end = header["latest_sesno"]
    window = aios_db.parse.collect_changes(db_file, start, end)
    total = sum(len(ops) for ops in window.values())
    print(f"  窗口 [{start}, {end}] 共 {total} 条元素操作，分布:")
    for sesno, ops in window.items():
        kinds = {}
        for op in ops:
            kinds[op["op"]] = kinds.get(op["op"], 0) + 1
        print(f"    sesno {sesno}: {kinds}")
    sample = next((op for ops in window.values() for op in ops), None)
    if sample is not None:
        print("  样例操作:", json.dumps(sample, ensure_ascii=False)[:300])
    # 一致性：所有操作的 sesno 都在窗口内
    for sesno, ops in window.items():
        assert start <= int(sesno) <= end
        for op in ops:
            assert start <= op["sesno"] <= end

    print("== db.connect + db.query（服务未跑则降级提示）==")
    try:
        # 配置已由开头的 set_config 指定；cwd 切到仓库根（resource/surreal 按 CWD 找）
        aios_db.connect(cwd=str(Path(__file__).resolve().parents[2]))
        rows = aios_db.db.query(
            "SELECT VALUE count() FROM pe WHERE dbnum = $dbnum GROUP ALL LIMIT 1;",
            {"dbnum": header["dbnum"]},
        )
        print(f"  pe 表中 dbnum={header['dbnum']} 的元素计数: {rows}")
    except RuntimeError as error:
        print(f"  [降级] 连接层不可用（大概率 SurrealDB 服务没在跑）: {error}")

    print("\nM1 冒烟全部通过。")


if __name__ == "__main__":
    main()
