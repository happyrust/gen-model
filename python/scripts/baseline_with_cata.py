# -*- coding: utf-8 -*-
"""基线一个 DESI 库；CATA 依赖默认**按需**，不预先整库解析。

生产链路自带元素级按需 CATA：生成模型时 `cata_closure`（`AIOS_CATA_CLOSURE_MODE`
缺省 On）对生成根的引用闭包做索引级定位 + 按需读取，配合运行期惰性兜底，
2GB 的 catalogue 文件只会被读到实际引用的那几条记录。因此**默认模式只基线
目标 DESI 库本身**——前提是 CATA 源文件在项目目录里可被发现（放着即可，
不解析）。

`--eager` 才走整库预热：把项目自己的 CATA 集按体积从小到大全量 baseline
（适合离线批量预热 / 与按需路径做对照）。依赖范围默认不跨项目——存在
dbnum 碰撞（AMS 的 ams7000 与 AvevaCatalogue 的 acp7000 都是 CATA
dbnum=7000），真实 MDB 里同一 dbnum 只有一份是成员；需要时用
`--cross-project` 显式打开。

用法（在 python/ 目录）：
    # 默认：只基线目标库，CATA 留给生成期按需解析
    .venv\\Scripts\\python.exe scripts\\baseline_with_cata.py \\
        --config .scratch\\DbOption-demo8043 --dbnum 7997
    # 整库预热对照
    .venv\\Scripts\\python.exe scripts\\baseline_with_cata.py \\
        --config .scratch\\DbOption-demo8043 --dbnum 7997 --eager [--dry-run]

- `--config` 决定连哪个 SurrealDB、扫哪个项目根（v_ip/v_port/project_path）。
- `--dry-run` 只做发现与计划，不 full_init、不写库。
- 执行态会拿项目单实例锁（full_init），务必确认目标 SurrealDB 是你自己的。
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import time
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = Path(__file__).resolve().parents[2]


def read_config_scope(config_path: Path) -> tuple[str, str, list[str]]:
    """从 DbOption toml 里抠 project_path / project_name / included_projects。

    用正则而不是完整 TOML 解析：配置文件由 Rust config crate 消费，这里只需要
    三个键，宽容处理注释与格式差异。
    """
    text = config_path.read_text(encoding="utf-8")

    def find(key: str) -> str:
        for line in text.splitlines():
            stripped = line.strip()
            if stripped.startswith("#"):
                continue
            match = re.match(rf"{key}\s*=\s*(.+)$", stripped)
            if match:
                return match.group(1).strip()
        raise SystemExit(f"配置 {config_path} 里找不到 {key}")

    project_path = find("project_path").strip('"')
    project_name = find("project_name").strip('"')
    included_raw = find("included_projects")
    included = re.findall(r'"([^"]+)"', included_raw)
    return project_path, project_name, included


def discover(parse_mod, project_root: Path, projects: list[str]) -> dict[tuple[str, int], dict]:
    """扫描项目库目录，返回 (project, dbnum) -> {type,path,sesno,size}。

    同一 (project, dbnum) 出现多个文件（复制残留）时取 latest_sesno 最大者。
    """
    found: dict[tuple[str, int], dict] = {}
    for project in projects:
        pdir = project_root / project
        if not pdir.exists():
            print(f"[发现] 项目目录不存在，跳过：{pdir}")
            continue
        for sub in sorted(pdir.glob("*000")):
            for file in sorted(sub.iterdir()):
                if not file.is_file():
                    continue
                try:
                    if not parse_mod.is_db_file(str(file)):
                        continue
                    header = parse_mod.header(str(file))
                except Exception as error:  # noqa: BLE001 - 单个坏文件只警告不中断
                    print(f"[发现] 跳过 {file.name}: {str(error)[:100]}")
                    continue
                key = (project, header["dbnum"])
                row = {
                    "type": header["db_type"],
                    "path": file,
                    "sesno": header["latest_sesno"],
                    "size": file.stat().st_size,
                }
                if key not in found or row["sesno"] > found[key]["sesno"]:
                    found[key] = row
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True,
                        help="DbOption 配置路径（可不带 .toml 后缀）")
    parser.add_argument("--dbnum", type=int, required=True, help="目标 DESI 库号")
    parser.add_argument("--project", default=None,
                        help="目标库所在项目名，缺省用配置的 project_name")
    parser.add_argument("--project-path", default=None,
                        help="E3D 项目根目录，缺省用配置的 project_path")
    parser.add_argument("--eager", action="store_true",
                        help="整库预热：把项目的 CATA 集全量 baseline（默认按需，不预解析）")
    parser.add_argument("--types", default="CATA",
                        help="eager 模式下视为依赖的库类型，逗号分隔（默认 CATA，可加 PADD,PROP,DICT）")
    parser.add_argument("--cross-project", action="store_true",
                        help="eager 依赖范围扩大到 included_projects 全部项目（注意 dbnum 碰撞）")
    parser.add_argument("--dry-run", action="store_true", help="只发现和出计划，不写库")
    parser.add_argument("--repo", type=Path, default=REPO_ROOT,
                        help="gen-model 仓库根（resource/surreal 按此 cwd 解析）")
    args = parser.parse_args()

    config_path = Path(args.config)
    if config_path.suffix != ".toml":
        config_path = config_path.with_suffix(".toml")
    config_path = config_path.resolve()
    assert config_path.exists(), f"配置不存在: {config_path}"

    import aios_db

    aios_db.set_config(str(config_path.with_suffix("")))

    cfg_root, cfg_project, cfg_included = read_config_scope(config_path)
    project_root = Path(args.project_path or cfg_root)
    target_project = args.project or cfg_project
    scan_projects = cfg_included if args.cross_project else [target_project]
    dep_types = {t.strip().upper() for t in args.types.split(",") if t.strip()}

    print(f"[发现] 项目根 {project_root}，扫描项目 {scan_projects}，依赖类型 {sorted(dep_types)}")
    catalog = discover(aios_db.parse, project_root, scan_projects)
    if not catalog:
        raise SystemExit("[发现] 没有扫到任何库文件，检查 project_path / included_projects")

    target_key = (target_project, args.dbnum)
    target = catalog.get(target_key)
    assert target, (
        f"目标库 dbnum={args.dbnum} 不在 {target_project} 的库目录里；"
        f"已发现 {len(catalog)} 个库文件"
    )

    deps = []
    if args.eager:
        deps = sorted(
            (
                (proj, dbnum, row)
                for (proj, dbnum), row in catalog.items()
                if row["type"] in dep_types and (proj, dbnum) != target_key
            ),
            key=lambda item: item[2]["size"],
        )

    print(f"\n[计划] 目标 {target_project}/{args.dbnum}（{target['type']}，"
          f"{target['size']:,} B，sesno {target['sesno']}）")
    if args.eager:
        print(f"[计划] eager 预热依赖 {len(deps)} 个，按体积从小到大先行入库：")
        total = 0
        for proj, dbnum, row in deps:
            total += row["size"]
            print(f"    {row['type']:5} {proj}/{dbnum:<8} sesno={row['sesno']:<6} "
                  f"{row['size']:>14,} B  {row['path'].name}")
        print(f"[计划] 依赖总体积 {total:,} B + 目标 {target['size']:,} B")
    else:
        present = [
            f"{proj}/{dbnum}({row['size']:,}B)"
            for (proj, dbnum), row in sorted(catalog.items())
            if row["type"] in dep_types
        ]
        print(f"[计划] 按需模式：只基线目标库；{len(present)} 个 {sorted(dep_types)} "
              f"源文件已就位（生成期由 cata_closure 按引用读取，不整库解析）：")
        for chunk in present:
            print(f"    {chunk}")

    if args.dry_run:
        print("\n[dry-run] 到此为止，未初始化、未写库。")
        return 0

    print("\n[执行] full_init（拿项目单实例锁，连接配置指定的 SurrealDB）…")
    started = time.time()
    aios_db.full_init(cwd=str(args.repo))
    print(f"[执行] full_init 完成（{time.time() - started:.1f}s）")

    reports = []
    for proj, dbnum, row in [*deps, (target_project, args.dbnum, target)]:
        started = time.time()
        report = aios_db.sync.baseline(dbnum, proj)
        elapsed = time.time() - started
        watermark = aios_db.db.watermark(dbnum)
        reports.append((proj, dbnum, row["type"], report, elapsed, watermark))
        print(f"[执行] baseline {proj}/{dbnum}（{row['type']}）完成 {elapsed:.1f}s，"
              f"watermark={watermark}，报告 {json.dumps(report, ensure_ascii=False, default=str)[:160]}")

    print("\n[汇总]")
    for proj, dbnum, dtype, report, elapsed, watermark in reports:
        print(f"    {dtype:5} {proj}/{dbnum:<8} {elapsed:6.1f}s watermark={watermark}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
