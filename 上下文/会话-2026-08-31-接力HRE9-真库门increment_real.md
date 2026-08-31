# 会话上下文 — 2026-08-31 · 接力 HRE9：e3d-model 增量更新的真库门

> 本会话：`BajieAsk-agent-1-b372a520`，12:18 从 HRE9（`BajieAsk-agent-1-f4518351`）接过存档。
> 上游链路：zhimo「fable-5-30」（Cursor composer `c7aedb14`）06:51 开工写增量更新，
> 08:36 被 zhimo 鉴权故障打断 → HRE9 11:36 恢复并核实 → 用户 11:40 拍板
> **「补真库门并验账」** → HRE9 写出 `tests/increment_real.rs` 后断在跑普查那一步。

## 接手时的现场

- `vendor/e3d-model` 工作树**干净**，比 HRE9 交接时多了一个 commit：
  `32ffc67 build: move to edition 2024, clear clippy and rustfmt, isolate target dir`
  —— 另一个会话把 crate 迁到 edition 2024，顺手把 HRE9 写的 `tests/increment_real.rs`
  一起收编了。所以断点产物没丢。
- `.cargo/config.toml` 里 `target-dir = "../../target-e3d-model"` 是**不生效**的：
  本机设着 `CARGO_TARGET_DIR` 环境变量指向共享池 `D:\Rust\target`，优先级更高。
  不显式覆盖就会跟别的会话抢同一个 target，实测直接 `LNK1104 无法打开 …exe`。
  **本 crate 跑 cargo 前一律先 `$env:CARGO_TARGET_DIR = "D:\work\plant-code\old\target-e3d-model"`。**

## 断点的真正难点：挑不到非空窗口

HRE9 留下的门本身是对的（口径见文件头），卡住的是**挑语料**：

1. 按索引候选数挑 → 全是空窗口。`ams8000` 最新窗口的 2 个修改、`ams6890` 最新窗口的
   258 个新增，折算到模型单元之后全落 `no_model`。真实语料里绝大多数会话只动非建模元素。
2. 改成对全语料跑 `plan_update` 普查（HRE9 的最后一条命令）→ **跑不动**。
   443 个库 2.4 GB，逐候选走 owner 链是随机读，实测 25 分钟只读了 734 MB、CPU 占用 10%，
   而且对无产出的库静默不打印，连进度都看不见。已停掉（PID 67020）。

## 本会话的解法：拿裁判去二分会话链

不再普查全语料，改成用**主门那位裁判**（两端全量生成的不变量快照）在单个库的会话链上二分：
链头与链尾各生成一次确认「这库的模型这辈子变过」，再对半收敛到相邻两环。
代价 `2 + log2(链长)` 次全量生成，几百环的链也就十来次。

落成两个 `#[ignore]` 探针（`PROBE_LIBS=xxx,yyy` 可换库）：

| 探针 | 干什么 | 适用 |
|---|---|---|
| `probe_chain_for_model_change` | 二分出「模型变过」的那一环 | 链长的库 |
| `probe_chain_profile` | 逐环画出全链的模型件数 | 链短的库（看件数在哪涨/跌） |

### 语料实况（探针实测）

- 443 个 `ams*_0001` 里**只有 67 个超过 1 MB**，绝大多数是空库——之前普查慢就是卡在
  少数几个巨型库上（`ams7351` 单个 1.18 GB）。
- 真正产模型的库更少。已探明：

| 库 | 链长 | 链头件数 | 备注 |
|---|---|---|---|
| `ams1112_0001` | 45 | 4476 | 会话 721 有 7824 件，722 只剩 4476 —— 一次删掉 3348 件 |
| `ams7999_0001` | 185 | 2101 | 窗口 45→46 是 2098 → 2101，**+3 插入** |
| `ams8000_0001` | 264 | 153 | 窗口 255→256 件数不变、内容变了 |
| `ams7997_0001` | 106 | 35202 | 窗口 73→74；每次全量 39s，太贵，没选 |
| `ams7350` / `ams7324` | 4 / 3 | 944 | 窗口是 0 → 944 全库新建，会撞「不许整库重建」那条断言，没选 |
| `ams5052/5053/5054/7000/7333/7355/7326/7322/7327/251xxx…` | — | **0** | 非设计内容，挑不出窗口 |

## 改动（只动了 `tests/increment_real.rs`）

1. **`SAMPLES` 从「取最新两环」改成钉死三个实测窗口**，各压一条路径：
   `ams7999 45→46`（插入）、`ams8000 255→256`（修改）、`ams1112 721→722`（删除）。
2. **形状不再是注释，是断言**：新增 `enum Shape`，样本声明压哪条路径，裁判就必须真的
   在那条路径上看见东西，看不见当场喊「这道门在空转」。
   —— 这条直接堵死了此前「全绿了几轮其实什么都没验」的坑。
3. **补了覆盖断言缺的反向一条**：原门只验「该删的删了没」，没验「删掉的是不是本该活着」。
   多重建只是白干活，多移除是把真实几何从模型集里抹掉，性质严重得多。
   现在断言 `removals ∩ target_full == ∅`。
4. 删掉跑不动的 `census_of_model_unit_windows` 与只它用的 `corpus()`；
   `newest_window` 换成 `chain_of`（返回整条链，二分要用）。

## 验证结果（12:5x 实测，全部真跑）

```
ams7999_0001（插入）: 45 -> 46   全量 2098 -> 2101 件（新增 3 变化 0 消失 0）；增量 upsert 3 remove 0
  candidates=23 (+22 ~1 -0) rolled_up=18 no_model=5 unresolved=0 cascades=0 units regen=3 remove=0 | regenerated=3 skipped=0 failed=0
ams8000_0001（修改）: 255 -> 256 全量 153 -> 153 件（新增 0 变化 1 消失 0）；增量 upsert 1 remove 0
  candidates=3 (+0 ~2 -1) rolled_up=3 no_model=0 unresolved=0 cascades=0 units regen=1 remove=0 | regenerated=1 skipped=0 failed=0
ams1112_0001（删除）: 721 -> 722 全量 7824 -> 4476 件（新增 0 变化 0 消失 3348）；增量 upsert 0 remove 3356
  candidates=24674 (+0 ~1 -24673) rolled_up=23469 no_model=1205 unresolved=0 cascades=0 units regen=0 remove=3356 | regenerated=0 skipped=0 failed=0
```

要点：

- **插入 / 修改两条路径零多算**：裁判说变 3 件就重建 3 件，说变 1 件就重建 1 件。
  不是「保守地整库重建也能过」，是精确命中。
- **删除路径 remove 3356 ⊇ vanished 3348**，多出的 8 件经反向断言证明**不在目标端全量里**
  ——是两端都没生成出几何的正体，属幂等「确保不存在」，不是误删活件。
- 三本账（候选侧 `rolled_up + no_model + unresolved == candidates`、执行侧
  `regenerated + skipped + failed == units_to_regenerate`）全部闭合。
- 三个窗口的 `unresolved` 与 `cascades` 都是 0 —— **变换级联那条路至今零真库覆盖**，见待办。

全量回归：`cargo test` → lib 61 + rvm_compare 6 + 真库门 1，全绿，耗时 23s；
`cargo fmt --check` 与 `cargo clippy --all-targets -- -D warnings` 均干净。

## 待办

- [ ] **级联零覆盖**：三个样本的 `cascades` 都是 0，「容器 POS/ORI 一动就重建整棵子树」
      这条与 Core3D 的有意分歧，至今没有一个真库窗口压过它。要么继续探链找带位移的窗口，
      要么造一个合成窗口。这是现在最大的验证缺口。
- [ ] `ams7997_0001`（35202 件、窗口 73→74）是目前最大的可用窗口，因单次全量 39s 没进
      `SAMPLES`；若要压规模可挂 `#[ignore]` 单独跑。
- [ ] 管件 noun（FTUB/BEND/ELBO/ATTA…）补进分类表 —— 需 aios-core 权威清单。
- [ ] 二期目录件（774 + 1157）规划。
- [ ] 双会话并发写：`.git` 治了「改动丢失」，同树无锁并发写（互踩）仍无机制；
      本轮另加一条 —— **共享 target 池也会互踩**（`LNK1104`），已用独占 target 目录规避。

## 工作日志

- 12:18 接 HRE9 存档，通读；核实盘面（工作树干净、多出 edition 2024 的 commit）
- 12:2x 发现 `CARGO_TARGET_DIR` 顶掉隔离配置，切独占 target 目录后编译通过
- 12:19–12:45 跑 HRE9 留下的全语料普查，25 分钟只啃动 1 个库 → 判定跑不动，停掉
- 12:4x 改用「裁判二分会话链」，ams8000 十次生成 3 秒锁定窗口，路子成立
- 12:5x 批量探库，摸清语料实况；定下三个钉死窗口
- 12:5x 改 `SAMPLES` + `enum Shape` 空窗口自检 + 补反向（多删）断言
- 12:5x 主门三条路径全绿；fmt / clippy / 全量回归全过；落本档案
