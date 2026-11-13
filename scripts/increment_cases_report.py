#!/usr/bin/env python3
"""把 tests/fixtures/increment_cases.json 渲成 markdown（计划 2026-10-25 §5.1）。

登记表是单一事实，这里只是它的读法：按组一张表 + 末尾一行总账（各状态计数、`has` 占比）。
不连库、不跑测试；测试是否存在由 tests/increment_cases_registry.rs 钉。

用法：
    python scripts/increment_cases_report.py                 # 打到 stdout
    python scripts/increment_cases_report.py --out <file.md> # 写文件
    python scripts/increment_cases_report.py --summary       # 只要总账行
"""
from __future__ import annotations

import argparse
import json
from collections import Counter, OrderedDict
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
REGISTRY = REPO / "tests" / "fixtures" / "increment_cases.json"

GROUP_TITLES = {
    "A": "变化分类与根归结（planner）",
    "B": "窗口与会话链（collector）",
    "C": "便宜路：刚体前移与单元级发布",
    "D": "新旧判据与登录",
    "E": "目录版本核对",
    "F": "落库、任务与重启",
    "G": "房间归属",
    "H": "失败与自愈",
    "I": "性能预算",
    "J": "服务面契约",
    "U": "原地宏对（run-owned 对象，E3D 真机）",
}
STATUS_LABEL = {"has": "有", "partial": "部分", "todo": "补", "corpus": "语料"}


def load(path: Path = REGISTRY) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def cell(text: str) -> str:
    return text.replace("|", "\\|")


def render_tests(tests: list[str]) -> str:
    return "<br>".join(f"`{t}`" for t in tests) if tests else "—"


def render_change_types(codes: list[str]) -> str:
    return " ".join(codes) if codes else "—"


def render(registry: dict) -> str:
    groups: "OrderedDict[str, list[dict]]" = OrderedDict()
    for case in registry["cases"]:
        groups.setdefault(case["group"], []).append(case)
    lines = [
        "# 增量更新自动化测试案例登记表（生成稿）",
        "",
        f"> 由 `scripts/increment_cases_report.py` 从 `{REGISTRY.relative_to(REPO).as_posix()}` 渲成；计划：`{registry['plan']}`。改表不改这里。",
        "",
    ]
    for group in sorted(groups):
        lines.append(f"## {group} · {GROUP_TITLES.get(group, '')}".rstrip(" ·"))
        lines.append("")
        lines.append("| ID | 层 | 案例 | 判据 | 变更类型 | 状态 | 承载 | 资产 | 关联 |")
        lines.append("|---|---|---|---|---|---|---|---|---|")
        for case in groups[group]:
            status = STATUS_LABEL.get(case["status"], case["status"])
            if case.get("stage"):
                status = f"{status}（{case['stage']}）"
            links = " ".join(case.get("issues", []) + case.get("adrs", []))
            lines.append(
                (
                    "| {id} | {layer} | {title} | {oracle} | {change_types} | {status} "
                    "| {tests} | {assets} | {links} |"
                ).format(
                    id=case["id"],
                    layer=case["layer"],
                    title=cell(case["title"]),
                    oracle=" ".join(case["oracle"]),
                    change_types=render_change_types(case.get("change_types", [])),
                    status=status,
                    tests=cell(render_tests(case.get("tests", []))),
                    assets=cell(render_tests(case.get("assets", []))),
                    links=cell(links) or "—",
                )
            )
        lines.append("")
    lines.append(summary_line(registry))
    lines.append("")
    return "\n".join(lines)


def summary_line(registry: dict) -> str:
    counts = Counter(case["status"] for case in registry["cases"])
    total = len(registry["cases"])
    covered = counts["has"] + counts["partial"]
    share = (100.0 * covered / total) if total else 0.0
    stages = Counter(case.get("stage") for case in registry["cases"] if case.get("stage"))
    stage_text = " ".join(f"{stage}:{n}" for stage, n in sorted(stages.items()))
    return (
        f"总账：{total} 条；有 {counts['has']} / 部分 {counts['partial']} / 补 {counts['todo']} / 语料 {counts['corpus']}；"
        f"有承载（有 + 部分）占 {share:.0f} %；待落地段 {stage_text}。"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--registry", type=Path, default=REGISTRY)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--summary", action="store_true", help="只打总账一行")
    args = parser.parse_args()
    registry = load(args.registry)
    text = summary_line(registry) + "\n" if args.summary else render(registry)
    if args.out:
        args.out.write_text(text, encoding="utf-8")
        print(f"wrote {args.out}")
    else:
        print(text, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
