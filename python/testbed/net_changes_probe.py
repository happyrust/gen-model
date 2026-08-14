# -*- coding: utf-8 -*-
"""db 文件 sesno 窗口净增删改探针：一条命令拿到 added / deleted / modified。

底层是 `aios_db.parse.net_changes`（会话索引差分：只靠文件本身判定，不查库、
不逐会话解析记录，耗时与窗口内会话数解耦）。`--verify` 再跑一遍旧的
`parse.collect_changes`（逐会话回放）并折成净集对拍——两套口径的措辞差异
（窗口内加了又删、删了又建）按存在性归一后逐 refno 比对，顺带报出两边耗时。

用法（venv 里跑，纯文件解析、不需要 SurrealDB）:
    .venv\\Scripts\\python.exe testbed\\net_changes_probe.py \
        --file "D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams8000_0001" \
        --from 100 --to 214 [--with-noun] [--verify] [--json out.json]

`--from` 缺省为 1（等价「相对空库的首次导入形状」）；`--to` 缺省为文件最新会话。
退出码：0 = 成功（--verify 时含对拍一致）；1 = 参数/文件错误；2 = 对拍不一致。
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "pysrc"))

import aios_db  # noqa: E402


def fold_replay_to_presence(window: dict) -> dict[str, str]:
    """把逐会话回放折成「存在性净三态」，与差分口径可比。

    规则（与 Rust 侧 live 对拍测试同源）：按会话序看每个 refno 的首末操作——
    首op是 add = 起点不在场，末op是 delete = 终点不在场；
    (不在场→在场)=added，(在场→不在场)=deleted，(在场→在场)=modified，
    (不在场→不在场)=窗口内自我抵消（不出现）。
    「删了又建」由此判 modified、「加了又删」不出现，与差分一致；差分对
    「改回原值」的元素仍会报 modified（记录被重写、位置必变），回放同样报——
    两边口径天然一致，无需白名单。
    """
    first_last: dict[str, tuple[str, str]] = {}
    for sesno in sorted(window, key=int):
        for op in window[sesno]:
            kind = op.get("op")
            if kind not in ("add", "modified", "deleted"):
                continue
            refno = op["refno"]
            if refno in first_last:
                first_last[refno] = (first_last[refno][0], kind)
            else:
                first_last[refno] = (kind, kind)

    net: dict[str, str] = {}
    for refno, (first, last) in first_last.items():
        present_at_start = first != "add"
        present_at_end = last != "deleted"
        if not present_at_start and present_at_end:
            net[refno] = "added"
        elif present_at_start and not present_at_end:
            net[refno] = "deleted"
        elif present_at_start and present_at_end:
            net[refno] = "modified"
        # 两端都不在场：窗口内自我抵消，不进净集。
    return net


def diff_to_classes(result: dict) -> dict[str, str]:
    classes: dict[str, str] = {}
    for kind in ("added", "deleted", "modified"):
        for entry in result[kind]:
            classes[entry["refno"]] = kind
    return classes


def main() -> int:
    parser = argparse.ArgumentParser(description="sesno 窗口净增删改探针（会话索引差分）")
    parser.add_argument("--file", required=True, help="dabacon 库文件路径")
    parser.add_argument("--from", dest="start", type=int, default=1, help="窗口起点（默认 1）")
    parser.add_argument("--to", dest="end", type=int, default=None, help="窗口终点（默认文件最新会话）")
    parser.add_argument("--with-noun", action="store_true", help="为每个条目解析类型名（每 refno 一次记录解析）")
    parser.add_argument("--verify", action="store_true", help="与逐会话回放折叠对拍")
    parser.add_argument("--json", dest="json_out", default=None, help="把差分结果原样写到 JSON 文件")
    parser.add_argument("--config", default=None, help="DbOption 配置（解析层深处读全局配置；默认仓库根 DbOption）")
    parser.add_argument("--limit", type=int, default=20, help="每类清单最多打印几条（默认 20，0 = 不打清单）")
    args = parser.parse_args()

    file = Path(args.file)
    if not file.is_file():
        print(f"找不到文件: {file}", file=sys.stderr)
        return 1

    repo = Path(__file__).resolve().parents[2]
    aios_db.set_config(args.config or str(repo / "DbOption"))

    header = aios_db.parse.header(str(file))
    end = args.end if args.end is not None else int(header["latest_sesno"])
    print(
        f"文件 {file.name}: db_type={header['db_type']} dbnum={header['dbnum']} "
        f"最新会话={header['latest_sesno']}；窗口 {args.start}..={end}"
    )

    t0 = time.perf_counter()
    result = aios_db.parse.net_changes(str(file), args.start, end, with_noun=args.with_noun)
    diff_ms = (time.perf_counter() - t0) * 1000

    counts = result["counts"]
    stats = result["stats"]
    print(
        f"净差分 {diff_ms:.0f}ms: added={counts['added']} deleted={counts['deleted']} "
        f"modified={counts['modified']}（base={result['base_sesno']} target={result['target_sesno']}，"
        f"目标侧读页 {stats['target']['pages_read']}，共享子树剪枝 {stats['shared_subtree_prunes']}）"
    )
    anomalies = {
        key: stats["target"].get(key, 0) + stats["base"].get(key, 0)
        for key in (
            "duplicate_child_pointers",
            "out_of_order_child_keys",
            "out_of_range_child_pointers",
            "out_of_range_leaf_entries",
            "unreadable_child_pages",
            "level_anomalies",
        )
    }
    noisy = {key: value for key, value in anomalies.items() if value}
    if noisy:
        print(f"结构观察（陈旧/残留条目均已按点查口径排除）: {noisy}")

    if args.limit:
        for kind in ("added", "deleted", "modified"):
            entries = result[kind]
            if not entries:
                continue
            shown = entries[: args.limit]
            rendered = ", ".join(
                e["refno"] + (f"({e['noun']})" if e.get("noun") else "")
                + (f"@{e['last_touch_sesno']}" if e.get("last_touch_sesno") is not None else "")
                for e in shown
            )
            more = f" …共 {len(entries)} 条" if len(entries) > len(shown) else ""
            print(f"  {kind}: {rendered}{more}")

    if args.json_out:
        Path(args.json_out).write_text(
            json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8"
        )
        print(f"结果已写入 {args.json_out}")

    if not args.verify:
        return 0

    t1 = time.perf_counter()
    window = aios_db.parse.collect_changes(str(file), args.start, end)
    replay_ms = (time.perf_counter() - t1) * 1000
    replay_net = fold_replay_to_presence(window)
    diff_net = diff_to_classes(result)

    mismatches = [
        (refno, diff_net.get(refno), replay_net.get(refno))
        for refno in sorted(set(replay_net) | set(diff_net))
        if diff_net.get(refno) != replay_net.get(refno)
    ]

    ops_total = sum(len(ops) for ops in window.values())
    speedup = replay_ms / diff_ms if diff_ms > 0 else float("inf")
    print(
        f"对拍：回放 {replay_ms:.0f}ms（{len(window)} 个会话 / {ops_total} 条 op，"
        f"折叠净集 {len(replay_net)} 条）vs 差分 {diff_ms:.0f}ms（净集 {len(diff_net)} 条），{speedup:.1f}x"
    )
    if not mismatches:
        print("对拍一致：逐 refno 净三态完全相同")
        return 0

    # 不一致 ≠ 差分错：逐会话回放的口径有已知盲区（临时 Add 被终态对账剔除后
    # 留下孤儿 Modified/Deleted 腿、跨会话删除漏报）。用生产 B+ 点查在窗口两端
    # 仲裁每一条：`parse.element(sesno=…)` 找不到即不在场，found_sesno 在窗口内
    # 变动即被重写。仲裁站差分一边的归「旧口径盲区」，站回放一边的才是缺陷。
    def presence_at(refno: str, sesno: int | None) -> tuple[bool, int | None]:
        """(在场与否, 版本会话号)。「索引可达但记录解析失败」也算在场——存在性
        以索引为准（错误文案区分两种 RuntimeError），版本号给 None。"""
        if sesno is None:
            return (False, None)
        try:
            dump = aios_db.parse.element(str(file), refno, sesno=sesno)
            return (True, int(dump["found_sesno"]))
        except RuntimeError as error:
            if "找不到" in str(error):
                return (False, None)
            return (True, None)

    base_sesno = result["base_sesno"]
    target_sesno = result["target_sesno"]
    blind_spots: dict[str, int] = {}
    failures: list[str] = []
    for refno, diff_class, replay_class in mismatches:
        before_present, before_ses = presence_at(refno, base_sesno)
        after_present, after_ses = presence_at(refno, target_sesno)
        if not before_present and after_present:
            acceptable = {"added"}
        elif before_present and not after_present:
            acceptable = {"deleted"}
        elif before_present and after_present:
            if before_ses is not None and after_ses is not None:
                acceptable = {"modified"} if before_ses != after_ses else {None}
            else:
                # 记录解析不出版本号：重写与否判不了，两种结论都接受。
                acceptable = {"modified", None}
        else:
            acceptable = {None}
        if diff_class in acceptable:
            key = f"回放折叠误报 {replay_class}（点查裁定 {sorted(str(a) for a in acceptable)}）"
            blind_spots[key] = blind_spots.get(key, 0) + 1
        else:
            failures.append(
                f"  {refno}: 差分={diff_class} 回放折叠={replay_class} "
                f"点查仲裁可接受={sorted(str(a) for a in acceptable)}"
            )

    if blind_spots:
        print(f"旧口径盲区（点查仲裁站差分一边）共 {sum(blind_spots.values())} 条：")
        for key, count in sorted(blind_spots.items()):
            print(f"  {count:>6} 条  {key}")
    if failures:
        print(f"仲裁后仍不一致 {len(failures)} 条（差分缺陷，需排查）：")
        print("\n".join(failures[:50]))
        if len(failures) > 50:
            print(f"  …共 {len(failures)} 条")
        return 2
    print("对拍收口：全部差异都由点查仲裁归因为旧口径盲区，差分与点查零分歧")
    return 0


if __name__ == "__main__":
    sys.exit(main())
