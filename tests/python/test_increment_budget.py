"""scripts/increment_budget.py 的离线测试：采样口径、判定规则，以及它吃的那几行服务日志在源码里还是这个格式。

不起服务、不连库；日志行取自 output/increment-replay 下的真实回放（ams8000-equi-move-chain / ams8000-equi-room-cross）。
"""

import contextlib
import importlib.util
import io
import json
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "increment_budget.py"
BASELINE_FILE = ROOT / "tests" / "fixtures" / "increment_budget_baseline.json"
SPEC = importlib.util.spec_from_file_location("increment_budget", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)

DB_OPTION = """
watch_dbnums = [
    8000,  # 被回放的库
]
"""

FIRST_PROCESS_LOG = """
[增量] 执行完毕 task=db-20261026-000331-000029 dbnum=8191 状态=succeeded 保存时间=未解析 会话区间=0..=891 新增=0 修改=0 删除=0 合计=0 总耗时=1500ms 完成时间=2026-10-26 00:03:33
[增量] 执行完毕 task=db-20261026-000331-000030 dbnum=8000 状态=succeeded 保存时间=未解析 会话区间=0..=725 新增=0 修改=0 删除=0 合计=0 总耗时=21503ms 完成时间=2026-10-26 00:04:40
[2026-10-26 00:04:41.958+08:00] [http.model.generate] op=1 refno=24384/24776 force=false
[2026-10-26 00:04:41.965+08:00] [处理结束] op=3 elapsed_ms=1 refno=24384_24776 generation_roots=1
[2026-10-26 00:04:42.067+08:00] [准备模型数据源] op=4 dbnum=8000 roots=["24384/24776"] frozen=false
[2026-10-26 00:04:42.364+08:00] [处理结束] op=4 elapsed_ms=296 dbnum=8000 completed=["24384/24776"] failures=[] rows=0 commits=0 room_edges=0
[2026-10-26 00:04:42.441+08:00] [处理结束] op=2 elapsed_ms=478 success refno=24384/24776
[2026-10-26 00:04:42.442+08:00] [处理结束] op=1 elapsed_ms=483 generated refno=24384/24776 available=true
[2026-10-26 00:04:42.443+08:00] [http.model.generate] op=5 refno=24384/24776 force=false
[2026-10-26 00:04:42.450+08:00] [处理结束] op=7 elapsed_ms=0 refno=24384_24776 generation_roots=1
[2026-10-26 00:04:42.485+08:00] [处理结束] op=6 elapsed_ms=36 success refno=24384/24776
[2026-10-26 00:04:42.485+08:00] [处理结束] op=5 elapsed_ms=42 generated refno=24384/24776 available=true
IncrementPipeline 阶段耗时 dbnum=8000 db_type=DESI 元素=1: collect=0ms plan=688ms cata_dependency=0ms persist=187ms cache=0ms rev_index=0ms datacenter=48ms finalize=15ms
[单元快照] 开工前 根=0 房间 0ms（拍到 0 根） 几何 0ms（拍到 0 根）
[增量] 执行完毕 task=db-20261026-000502-000047 dbnum=8000 状态=succeeded 保存时间=2026-10-25T13:48:27+00:00 会话区间=726..=726 新增=0 修改=1 删除=0 合计=1 总耗时=2535ms 完成时间=2026-10-26 00:05:05
[增量] 执行完毕 task=db-20261026-000532-000048 dbnum=8000 状态=failed 保存时间=2026-10-25T13:48:30+00:00 会话区间=727..=727 新增=0 修改=1 删除=0 合计=1 总耗时=21027ms 完成时间=2026-10-26 00:05:53
IncrementPipeline 阶段耗时 dbnum=8000 db_type=DESI 元素=2: collect=0ms plan=692ms cata_dependency=0ms persist=200ms cache=0ms rev_index=0ms datacenter=49ms finalize=15ms
IncrementPipeline 阶段耗时 dbnum=7997 db_type=DESI 元素=1: collect=0ms plan=13800ms cata_dependency=0ms persist=190ms cache=0ms rev_index=0ms datacenter=20ms finalize=15ms
[单元快照] 开工前 根=1 房间 30ms（拍到 1 根） 几何 0ms（拍到 1 根）
[单元快照] 收尾后 根=1 几何 0ms（房间的「后」等刷盘之后再拍）
[单元快照] 刷盘后 根=1 房间 29ms（拍到 1 根，缺 0）
[增量] 执行完毕 task=db-20261026-000602-000049 dbnum=8000 状态=succeeded 保存时间=2026-10-25T13:48:35+00:00 会话区间=727..=728 新增=0 修改=2 删除=0 合计=2 总耗时=2687ms 完成时间=2026-10-26 00:06:05
[2026-10-26 00:06:40.100+08:00] [http.model.generate] op=9 refno=24384/23257 force=false
"""

# 冷重启后的进程：op 号从 1 重数；房间的「后」还是 #71 之前「收尾后 … 房间 X ms」的老写法。
RESTARTED_PROCESS_LOG = """
[2026-10-26 00:07:13.793+08:00] [http.model.generate] op=1 refno=24384/24776 force=false
[2026-10-26 00:07:13.835+08:00] [处理结束] op=1 elapsed_ms=41 generated refno=24384/24776 available=true
[单元快照] 开工前 根=1 房间 31ms（拍到 1 根） 几何 0ms（拍到 1 根）
[单元快照] 收尾后 根=1 房间 26ms 几何 0ms
[增量] 执行完毕 task=db-20261026-000815-000001 dbnum=8000 状态=succeeded 保存时间=未解析 会话区间=0..=728 新增=0 修改=0 删除=0 合计=0 总耗时=22942ms 完成时间=2026-10-26 00:08:38
[增量] 执行完毕 task=db-20261026-000915-000003 dbnum=8000 状态=succeeded 保存时间=2026-10-25T13:48:46+00:00 会话区间=730..=730 新增=0 修改=1 删除=0 合计=1 总耗时=2502ms 完成时间=2026-10-26 00:09:18
"""

BASELINE = {
    "metrics": {
        "plan_ms": {"baseline_ms": 760, "limit_ms": 1500, "required": True, "max_elements": 2},
        "room_snapshot_ms_per_root": {"baseline_ms": 30, "limit_ms": 200, "required": True},
        "generate_cached_ms": {"baseline_ms": 50, "limit_ms": 250, "required": False},
        "generate_rebuild_ms": {"baseline_ms": 700, "limit_ms": None, "required": False, "gate": False},
    }
}


def collect_fixture(dbnums=None):
    with tempfile.TemporaryDirectory() as td:
        run = Path(td)
        (run / "DbOption.toml").write_text(DB_OPTION, encoding="utf-8")
        (run / "stdout.log").write_text(FIRST_PROCESS_LOG, encoding="utf-8")
        (run / "stdout-restart1.log").write_text(RESTARTED_PROCESS_LOG, encoding="utf-8")
        return MODULE.collect(run, "fixture", dbnums)


def values(budget, metric):
    return sorted(sample["value_ms"] for sample in budget["samples"] if sample["metric"] == metric)


def sample(metric, value, **extra):
    return {"metric": metric, "value_ms": value, **extra}


def budget_of(*samples):
    return {"case": "t", "samples": list(samples)}


class CollectTests(unittest.TestCase):
    def test_samples_follow_the_metric_definitions(self):
        budget = collect_fixture()
        self.assertEqual(budget["dbnums"], [8000])
        # 7997 不在 watch_dbnums 里；增量窗只算 succeeded 且起点 > 1 的（0..=N 是首载 / 整库重建）。
        self.assertEqual(values(budget, "plan_ms"), [688, 692])
        self.assertEqual(values(budget, "window_batch_ms"), [2502, 2535, 2687])
        # 根=0 不计；「刷盘后」与老写法「收尾后 … 房间」都认，新写法的「收尾后」只剩几何、不计。
        self.assertEqual(values(budget, "room_snapshot_ms_per_root"), [26, 29, 30, 31])
        # 中间开了 [准备模型数据源] 的是真要算；进程被杀时还没结束的那次不计。
        self.assertEqual(values(budget, "generate_rebuild_ms"), [483])
        self.assertEqual(values(budget, "generate_cached_ms"), [41, 42])

    def test_window_samples_carry_their_size(self):
        windows = {
            sample["window"]: (sample["elements"], sample["sessions"])
            for sample in collect_fixture()["samples"]
            if sample["metric"] == "window_batch_ms"
        }
        self.assertEqual(windows, {"726..=726": (1, 1), "727..=728": (2, 2), "730..=730": (1, 1)})

    def test_explicit_dbnums_override_the_run_config(self):
        self.assertEqual(values(collect_fixture({8000, 7997}), "plan_ms"), [688, 692, 13800])

    def test_every_watched_dbnum_counts(self):
        with tempfile.TemporaryDirectory() as td:
            run = Path(td)
            self.assertIsNone(MODULE.read_dbnums(run))
            (run / "DbOption.toml").write_text(
                """
project = 'AvevaMarineSample'
watch_dbnums = [
    8000,  # 被回放的库
    7997,
]
""",
                encoding="utf-8",
            )
            self.assertEqual(MODULE.read_dbnums(run), {8000, 7997})


class CheckTests(unittest.TestCase):
    def test_a_required_metric_without_samples_fails(self):
        fails, warns, oks, notes = MODULE.check([budget_of(sample("room_snapshot_ms_per_root", 30))], BASELINE)
        self.assertEqual(len(fails), 1)
        self.assertIn("plan_ms: 没有样本", fails[0])
        self.assertEqual((len(warns), len(oks), len(notes)), (0, 1, 0))

    def test_windows_wider_than_the_baseline_are_left_out(self):
        fails, warns, oks, notes = MODULE.check([budget_of(
            sample("plan_ms", 700, elements=1),
            sample("plan_ms", 9000, elements=40),
            sample("room_snapshot_ms_per_root", 30),
        )], BASELINE)
        self.assertEqual((fails, warns), ([], []))
        plan = [line for line in oks if "plan_ms" in line]
        self.assertEqual(len(plan), 1)
        self.assertIn("n=1 中位数 700.0 ms", plan[0])
        self.assertIn("另有 1 个超过 2 元素的窗只记不判", plan[0])

    def test_only_wide_windows_warn_instead_of_failing(self):
        fails, warns, oks, notes = MODULE.check([budget_of(
            sample("plan_ms", 9000, elements=40),
            sample("room_snapshot_ms_per_root", 30),
        )], BASELINE)
        self.assertEqual(fails, [])
        self.assertEqual(len(warns), 1)
        self.assertIn("plan_ms: 1 个样本全是超过 2 元素的窗", warns[0])

    def test_a_record_only_metric_is_never_judged(self):
        fails, warns, oks, notes = MODULE.check([budget_of(
            sample("plan_ms", 760, elements=1),
            sample("room_snapshot_ms_per_root", 30),
            sample("generate_rebuild_ms", 1269),
        )], BASELINE)
        self.assertEqual((fails, warns), ([], []))
        self.assertEqual(len(notes), 1)
        self.assertIn("generate_rebuild_ms", notes[0])
        self.assertIn("只记不判", notes[0])

    def test_median_limit_and_idle_rules(self):
        fails, warns, oks, notes = MODULE.check([budget_of(
            sample("plan_ms", 1200, elements=1),
            sample("room_snapshot_ms_per_root", 30),
            sample("room_snapshot_ms_per_root", 30),
            sample("room_snapshot_ms_per_root", 250),
            sample("generate_cached_ms", 10),
        )], BASELINE)
        self.assertEqual(len(fails), 2)
        self.assertTrue(any("plan_ms" in line and "中位数 1200.0 > 基线 760.0 × 1.5" in line for line in fails))
        self.assertTrue(any("room_snapshot_ms_per_root" in line and "最大值 250.0 > 绝对上限 200" in line for line in fails))
        self.assertEqual(len(warns), 1)
        self.assertIn("generate_cached_ms", warns[0])
        self.assertIn("可能是没干活", warns[0])

    def test_a_run_shaped_like_the_recorded_corpus_passes_the_committed_baseline(self):
        baseline = json.loads(BASELINE_FILE.read_text(encoding="utf-8"))
        fails, warns, oks, notes = MODULE.check([collect_fixture()], baseline)
        self.assertEqual((fails, warns), ([], []))
        self.assertEqual(len(oks), 4)
        self.assertEqual(len(notes), 1)
        self.assertIn("generate_rebuild_ms", notes[0])

    def test_check_command_exit_code(self):
        with tempfile.TemporaryDirectory() as td:
            folder = Path(td)
            baseline = folder / "baseline.json"
            baseline.write_text(json.dumps(BASELINE), encoding="utf-8")
            good = folder / "good.json"
            good.write_text(json.dumps(budget_of(
                sample("plan_ms", 760, elements=1), sample("room_snapshot_ms_per_root", 30))), encoding="utf-8")
            bad = folder / "bad.json"
            bad.write_text(json.dumps(budget_of(sample("room_snapshot_ms_per_root", 30))), encoding="utf-8")
            with contextlib.redirect_stdout(io.StringIO()) as out:
                self.assertEqual(MODULE.main(["check", str(good), "--baseline", str(baseline)]), 0)
                self.assertEqual(MODULE.main(["check", str(bad), "--baseline", str(baseline)]), 1)
        self.assertIn("预算门：ok 2 / warn 0 / fail 0 / 只记 0", out.getvalue())
        self.assertIn("FAIL [t] plan_ms: 没有样本", out.getvalue())


class ServiceLogShapeTests(unittest.TestCase):
    """预算脚本的正则吃的是服务自己打的这几行；源码里的格式串一改，这里先红，提醒同时改 scripts/increment_budget.py。"""

    SHAPES = {
        "src/data_interface/increment_pipeline.rs": [
            "IncrementPipeline 阶段耗时 dbnum={dbnum} db_type={db_type} 元素={elements}: ",
            "collect={}ms plan={}ms",
        ],
        "src/data_interface/batch_worker.rs": [
            "[增量] 执行完毕 task={task_id} dbnum={dbnum} 状态={state} ",
            "会话区间={start_sesno}..={end_sesno} ",
            "合计={} 总耗时={total_ms}ms",
            "[单元快照] 开工前 根={} 房间 {}ms（拍到 {} 根） 几何 {}ms",
            "[单元快照] 刷盘后 根={} 房间 {}ms（拍到 {} 根",
        ],
        "src/process_diagnostics.rs": [
            '"[{}] [{}] op={} {}"',
            '"elapsed_ms={} {}"',
            '"model.source.begin" => "准备模型数据源"',
            '"finished" => "处理结束"',
        ],
        "src/data_interface/model_refresh.rs": [
            'Operation::begin("model.source.begin"',
            "dbnum={dbnum} roots={roots:?}",
        ],
        "src/web_service/handlers.rs": [
            'Operation::begin("http.model.generate"',
            '"refno={} force={}"',
            '"generated refno={} available={}"',
        ],
    }

    def test_the_log_lines_the_budget_parses_are_still_printed(self):
        for relative, fragments in self.SHAPES.items():
            source = (ROOT / relative).read_text(encoding="utf-8")
            for fragment in fragments:
                with self.subTest(file=relative, fragment=fragment):
                    self.assertIn(fragment, source, "服务日志的格式变了，scripts/increment_budget.py 的正则要一起改")

    def test_the_generate_request_keeps_its_raw_stage_name_on_the_console(self):
        # console_label 给 http.model.generate 配了中文名，控制台那一行就不再是 `[http.model.generate]`。
        source = (ROOT / "src" / "process_diagnostics.rs").read_text(encoding="utf-8")
        self.assertNotIn('"http.model.generate" =>', source)


if __name__ == "__main__":
    unittest.main()
