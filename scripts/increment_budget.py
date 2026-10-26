#!/usr/bin/env python3
"""增量性能预算（计划 2026-10-25 §5.4 / INC-I01 I02 I04 I05）：从一次 L2 回放运行目录里把服务日志解成
`{metric, value_ms, …}` 样本，写成 `output/increment-budget/<stamp>.json`；再拿基线文件对：
「中位数超基线 50 % 或最大值超绝对上限即红；中位数低于基线 50 % 打 WARN（可能是没干活）」。

只读日志、不连库、不起进程；量的是服务自己打的账（`IncrementPipeline 阶段耗时`、`[单元快照]`、`[增量] 执行完毕 … 总耗时`、
`[http.model.generate]` → `[处理结束] … elapsed_ms`），和 harness / l3_suite 怎么跑无关。

指标（都是毫秒）：
- `plan_ms`               窗口规划器 `plan=`（INC-I02），每个增量窗一样；
- `window_batch_ms`       增量窗批次 `总耗时`（INC-I04 的批次半边；不含监听器轮询与排队），只计 succeeded 的增量窗（起点 > 1）；
- `room_snapshot_ms_per_root`  `[单元快照] 开工前 / 收尾后` 的房间毫秒 ÷ 根数（INC-I01），根数 0 不计；
- `generate_cached_ms`    未触根 `/model/generate` 往返（INC-I05）：`[http.model.generate]` 到同 op 的 `[处理结束]` 之间
                          没有 `[准备模型数据源] … roots=[这根]` 的那些（凭证命中也有 `[模型后台任务开始]`，
                          只有真要算的才开数据源）；
- `generate_rebuild_ms`   有后台任务的（补常驻根 / 真生成），只记不判。

用法：
    python scripts/increment_budget.py collect <run_dir> [--case NAME] [--dbnum N] [--out FILE]
    python scripts/increment_budget.py check <budget.json>... [--baseline tests/fixtures/increment_budget_baseline.json]
    python scripts/increment_budget.py collect <run_dir> --check          # 采完顺手对基线

退出码：check 有 FAIL 回 1，否则 0（WARN 不影响退出码）。
"""
from __future__ import annotations

import argparse
import json
import re
import statistics
import sys
from datetime import datetime
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
DEFAULT_BASELINE = REPO / "tests" / "fixtures" / "increment_budget_baseline.json"
DEFAULT_OUT_DIR = REPO / "output" / "increment-budget"
FORMAT = "increment-budget-v1"

RE_PLAN = re.compile(
    r"IncrementPipeline 阶段耗时 dbnum=(?P<dbnum>\d+) db_type=(?P<db_type>\w+) 元素=(?P<elements>\d+): "
    r"collect=(?P<collect>\d+)ms plan=(?P<plan>\d+)ms"
)
RE_TOTAL = re.compile(
    r"\[增量\] 执行完毕 task=(?P<task>\S+) dbnum=(?P<dbnum>\d+) 状态=(?P<state>\w+) .*?"
    r"会话区间=(?P<start>\d+)\.\.=(?P<end>\d+) .*?总耗时=(?P<total>\d+)ms"
)
RE_SNAP_BEFORE = re.compile(
    r"\[单元快照\] 开工前 根=(?P<roots>\d+) 房间 (?P<room>\d+)ms（拍到 (?P<room_roots>\d+) 根） 几何 (?P<geom>\d+)ms"
)
RE_SNAP_AFTER = re.compile(r"\[单元快照\] 收尾后 根=(?P<roots>\d+) 房间 (?P<room>\d+)ms 几何 (?P<geom>\d+)ms")
RE_GEN_START = re.compile(r"\[http\.model\.generate\] op=(?P<op>\d+) refno=(?P<refno>\S+)")
RE_GEN_PREPARE = re.compile(r"\[准备模型数据源\] op=\d+ dbnum=\d+ roots=\[(?P<roots>[^\]]*)\]")
RE_GEN_END = re.compile(r"\[处理结束\] op=(?P<op>\d+) elapsed_ms=(?P<ms>\d+) generated refno=(?P<refno>\S+)")


def read_dbnum(run_dir: Path) -> int | None:
    config = run_dir / "DbOption.toml"
    if not config.is_file():
        return None
    match = re.search(r"^\s*watch_dbnums\s*=\s*\[\s*(\d+)", config.read_text(encoding="utf-8", errors="replace"), re.M)
    return int(match.group(1)) if match else None


def service_logs(run_dir: Path) -> list[Path]:
    # stdout.log 是第一次起的进程，stdout-restartN.log 是冷重启后的（INC-F02）；都算。
    return sorted(run_dir.glob("stdout*.log"))


def collect(run_dir: Path, case: str | None, dbnum: int | None) -> dict:
    dbnum = dbnum or read_dbnum(run_dir)
    samples: list[dict] = []
    for log in service_logs(run_dir):
        pending_generate: dict[str, dict] = {}
        with log.open(encoding="utf-8", errors="replace") as handle:
            for lineno, line in enumerate(handle, 1):
                line = line.rstrip("\n")
                where = f"{log.name}:{lineno}"
                if (m := RE_PLAN.search(line)) and (dbnum is None or int(m["dbnum"]) == dbnum):
                    samples.append({"metric": "plan_ms", "value_ms": int(m["plan"]), "elements": int(m["elements"]),
                                    "dbnum": int(m["dbnum"]), "at": where})
                    continue
                if (m := RE_TOTAL.search(line)) and (dbnum is None or int(m["dbnum"]) == dbnum):
                    start, end = int(m["start"]), int(m["end"])
                    if m["state"] == "succeeded" and start > 1:
                        samples.append({"metric": "window_batch_ms", "value_ms": int(m["total"]),
                                        "window": f"{start}..={end}", "task": m["task"], "dbnum": int(m["dbnum"]), "at": where})
                    continue
                if m := RE_SNAP_BEFORE.search(line):
                    roots = int(m["roots"])
                    if roots > 0:
                        samples.append({"metric": "room_snapshot_ms_per_root", "value_ms": int(m["room"]) / roots,
                                        "phase": "before", "roots": roots, "at": where})
                    continue
                if m := RE_SNAP_AFTER.search(line):
                    roots = int(m["roots"])
                    if roots > 0:
                        samples.append({"metric": "room_snapshot_ms_per_root", "value_ms": int(m["room"]) / roots,
                                        "phase": "after", "roots": roots, "at": where})
                    continue
                if m := RE_GEN_START.search(line):
                    pending_generate[m["op"]] = {"refno": m["refno"], "background": False, "at": where}
                    continue
                if m := RE_GEN_PREPARE.search(line):
                    # 数据源的 op 是自己的号，按 roots 里的 refno 把正在等的那次 generate 标成「真要算」。
                    roots = {root.strip().strip('"') for root in m["roots"].split(",")}
                    for entry in pending_generate.values():
                        if entry["refno"] in roots:
                            entry["background"] = True
                    continue
                if m := RE_GEN_END.search(line):
                    entry = pending_generate.pop(m["op"], None)
                    if entry is None:
                        continue
                    metric = "generate_rebuild_ms" if entry["background"] else "generate_cached_ms"
                    samples.append({"metric": metric, "value_ms": int(m["ms"]), "refno": m["refno"], "at": where})
    return {
        "format": FORMAT,
        "run_dir": str(run_dir),
        "case": case or run_dir.name,
        "dbnum": dbnum,
        "collected_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "summary": summarize(samples),
        "samples": samples,
    }


def summarize(samples: list[dict]) -> dict:
    by_metric: dict[str, list[float]] = {}
    for sample in samples:
        by_metric.setdefault(sample["metric"], []).append(float(sample["value_ms"]))
    summary = {}
    for metric, values in sorted(by_metric.items()):
        values.sort()
        summary[metric] = {
            "n": len(values),
            "min_ms": round(values[0], 1),
            "median_ms": round(statistics.median(values), 1),
            "max_ms": round(values[-1], 1),
        }
    return summary


def check(budgets: list[dict], baseline: dict) -> tuple[list[str], list[str], list[str]]:
    """回 (fails, warns, oks)，每条一句话。基线没有的指标只记不判；运行里没样本的指标打 WARN（可能没干活）。"""
    fails, warns, oks = [], [], []
    for budget in budgets:
        label = budget.get("case", "?")
        summary = budget.get("summary", {})
        for metric, rule in baseline["metrics"].items():
            if metric not in summary:
                if rule.get("required", True):
                    warns.append(f"[{label}] {metric}: 没有样本（这次运行没走到这条路？）")
                continue
            stats = summary[metric]
            base = float(rule["baseline_ms"])
            limit = rule.get("limit_ms")
            median, maximum = stats["median_ms"], stats["max_ms"]
            verdict = "ok"
            why = []
            if median > base * 1.5:
                verdict = "fail"
                why.append(f"中位数 {median} > 基线 {base} × 1.5")
            if limit is not None and maximum > float(limit):
                verdict = "fail"
                why.append(f"最大值 {maximum} > 绝对上限 {limit}")
            if verdict == "ok" and median < base * 0.5:
                verdict = "warn"
                why.append(f"中位数 {median} < 基线 {base} × 0.5，可能是没干活")
            line = (f"[{label}] {metric}: n={stats['n']} 中位数 {median} ms 最大 {maximum} ms"
                    f"（基线 {base}，上限 {limit}）" + ("：" + "；".join(why) if why else ""))
            {"fail": fails, "warn": warns, "ok": oks}[verdict].append(line)
    return fails, warns, oks


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    collect_cmd = sub.add_parser("collect", help="从运行目录采预算样本")
    collect_cmd.add_argument("run_dir", type=Path)
    collect_cmd.add_argument("--case", help="用例名（缺省取运行目录名）")
    collect_cmd.add_argument("--dbnum", type=int, help="只计这个库（缺省读运行目录里 DbOption.toml 的 watch_dbnums）")
    collect_cmd.add_argument("--out", type=Path, help="写到这个文件（缺省 output/increment-budget/<case>-<stamp>.json）")
    collect_cmd.add_argument("--check", action="store_true", help="采完顺手对基线")
    collect_cmd.add_argument("--baseline", type=Path, default=DEFAULT_BASELINE)

    check_cmd = sub.add_parser("check", help="拿预算 JSON 对基线")
    check_cmd.add_argument("budgets", nargs="+", type=Path)
    check_cmd.add_argument("--baseline", type=Path, default=DEFAULT_BASELINE)

    args = parser.parse_args(argv)

    if args.command == "collect":
        run_dir = args.run_dir.resolve()
        if not run_dir.is_dir():
            print(f"运行目录不存在：{run_dir}", file=sys.stderr)
            return 2
        budget = collect(run_dir, args.case, args.dbnum)
        out = args.out
        if out is None:
            DEFAULT_OUT_DIR.mkdir(parents=True, exist_ok=True)
            stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
            out = DEFAULT_OUT_DIR / f"{budget['case']}-{stamp}.json"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(budget, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"预算样本 {len(budget['samples'])} 条 → {out}")
        for metric, stats in budget["summary"].items():
            print(f"  {metric}: n={stats['n']} min {stats['min_ms']} 中位 {stats['median_ms']} max {stats['max_ms']} ms")
        if not args.check:
            return 0
        budgets = [budget]
    else:
        budgets = [json.loads(path.read_text(encoding="utf-8")) for path in args.budgets]

    baseline = json.loads(args.baseline.read_text(encoding="utf-8"))
    fails, warns, oks = check(budgets, baseline)
    for line in oks:
        print("OK   " + line)
    for line in warns:
        print("WARN " + line)
    for line in fails:
        print("FAIL " + line)
    print(f"预算门：ok {len(oks)} / warn {len(warns)} / fail {len(fails)}（基线 {args.baseline.name}）")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
