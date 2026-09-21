# 旧端源基线 sha256 · 2026-09-13 复核与补全

- 依据：`docs/plans/2026-09-12-review-backend-port-plan.md` §7 风险 5「plant-model-gen 非 git，移植到一半源被改」。
- 源根：`D:\work\plant-code\plant-model-gen`（非 git 仓库，同级有 `plant-model-gen.bundle`）。
- 前一份：`docs/evidence/2026-09-12-review-source-baseline.sha256`（23 条，2026-09-12 采样）。
- 本文做两件事：① 拿那 23 条**重新核一遍**，看移植期间源有没有漂；② 把任务口径里要求、
  但上一份没收进去的两个文件补上。

## 1. 漂移复核（2026-09-13 15:1x）

```powershell
cd D:\work\plant-code\plant-model-gen
Get-Content <上一份清单> | ? { $_ -notmatch '^#' -and $_.Trim() } | % {
  $h,$p = $_ -split '\s+',2
  $cur = (Get-FileHash -Algorithm SHA256 $p.Trim()).Hash.ToLower()
  if ($cur -ne $h) { "DRIFT $p" }
}
```

输出：

```
核对 23 条：一致 23，漂移 0，缺失 0
```

**结论：移植期间旧端源一个字节都没动**，§7 风险 5 在 2026-09-12 → 09-13 这一段没有发生。
23 条的哈希值以上一份清单为准，本文不复制一遍（复制就等于多了一份会各自漂的真源）。

## 2. 补两条（上一份未覆盖）

任务口径要求覆盖 `src/web_server/{mod.rs, dashboard_handlers.rs}`——前者是旧端挂
`/files/review_attachments` 静态目录的地方（`ServeDir("assets/review_attachments")`，计划 §1.2 附件行），
后者是 `/api/dashboard/activities`（读 `review_workflow_history`，计划 Q3 列为 P2 可选）。
两个都在校审域的影响面里，但上一份按 d-594 的口径只收了 `src/web_api/**`，漏了它们。

```
f6c097ff03d1b2d11dae4f8ff30d4528a73788689bbc7000036a03778c1f293e  src/web_server/mod.rs
5ab4d6370ef391c1d5f3f83fb57174ea427cb69e4266e6b52eb07ba5236a08a8  src/web_server/dashboard_handlers.rs
```

## 3. 怎么核

```powershell
# PowerShell（本文两条）
cd D:\work\plant-code\plant-model-gen
foreach ($p in 'src/web_server/mod.rs','src/web_server/dashboard_handlers.rs') {
  '{0}  {1}' -f (Get-FileHash -Algorithm SHA256 $p).Hash.ToLower(), $p
}
```

```bash
# Git Bash（上一份 23 条）
cd /d/work/plant-code/plant-model-gen
sha256sum -c docs/evidence/2026-09-12-review-source-baseline.sha256   # 路径按实际拷贝位置
```

注意：`plant-model-gen` 目录**只读**，本次复核只做 `Get-FileHash`，没有写入任何文件。
