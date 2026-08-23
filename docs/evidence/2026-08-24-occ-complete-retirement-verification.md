# OCC 完全退役源码验证记录（2026-08-24）

## 输入与版本

- 基线：`cf7ec05d0f88bacef4fe98a24dd02f7f40924e5a`
- 工作分支：`codex/occ-retire-endgame`
- aios-core：`f9551ef407f4c2c870405c612e4260ce9e091416`
- parse-pdms-db：`ac85df94a788c3bc5011bf66aec0872a7a729b46`
- pdms-io：`c4f02e97a36c0d5660a2aeab5b781b72f92144b1`

## 字面验证结果

以下命令均在 `D:\work\plant-code\old\gen-model-occ-retire-endgame` 执行。

### 依赖和 feature

```text
$ cargo tree -i aios_core@0.2.0 --depth 1
aios_core v0.2.0 (https://github.com/happyrust/old-aios-core.git?rev=f9551ef407f4c2c870405c612e4260ce9e091416#f9551ef4)
├── aios-database v0.1.18 (...)
├── parse_pdms_db v3.0.0 (...ac85df94...)
└── pdms_io v0.1.0 (...c4f02e97...)
exit status: 0

$ python <Cargo.lock/Cargo.toml/python-Cargo.toml OCC dependency scan>
OCC dependency references: 0
exit status: 0

$ python <cargo metadata root feature assertion>
root occ feature: False
exit status: 0

$ python <full cargo metadata aios_core source count>
aios_core packages: 1
('aios_core', 'git+https://github.com/happyrust/old-aios-core.git?rev=f9551ef407f4c2c870405c612e4260ce9e091416#f9551ef407f4c2c870405c612e4260ce9e091416')
exit status: 0
```

### 构建和测试

```text
$ cargo fmt --check
exit status: 0

$ cargo check --locked --no-default-features --features ws,gen_model,manifold,project_hd,http_api
cargo check: 0 errors, 4 warnings
exit status: 0

$ cargo check --locked
cargo check: 0 errors, 4 warnings
exit status: 0

$ cargo test --locked --lib --no-default-features --features ws,gen_model,manifold,project_hd
test result: ok. 1091 passed; 0 failed; 85 ignored
exit status: 0

$ cargo test --locked --test db8000_two_delete_fixture --no-default-features --features ws,gen_model,manifold,project_hd
test result: ok. 6 passed; 0 failed
exit status: 0

$ cargo test --locked --test db_session_fixture_selfcheck --no-default-features --features ws,gen_model,manifold,project_hd
test result: ok. 15 passed; 0 failed
exit status: 0

$ cargo test --locked --test db8000_session_pairs --no-default-features --features ws,gen_model,manifold,project_hd
test result: ok. 21 passed; 0 failed
exit status: 0

$ cargo test --locked --test pdms_record_boundary --no-default-features --features ws,gen_model,manifold,project_hd
test result: ok. 3 passed; 0 failed
exit status: 0

$ python/.venv/Scripts/python.exe -m pytest -m offline -q
85 passed, 23 deselected in 5.67s
exit status: 0
```

### 行为门

```text
$ cargo test ... rm12_smooth_surface_regression
test result: ok. 1 passed; 0 failed
exit status: 0

$ cargo test ... gen_inst_meshes_bails_without_backend_and_tries_libgm_first
test result: ok. 1 passed; 0 failed
exit status: 0
```

由此确认两种行为：启用 Manifold 时，轮廓硬边/光顺组通过属性顶点传播；不启用网格后端时，
模型生成以明确错误和失败回执终止，不静默跳过。

## Vendor 验证

aios-core 的默认检查、`--no-default-features --features gen_model,manifold,sql` 检查和
4 条 caliber 定向测试通过。其全量 `--lib` 运行结果为 112 通过、59 失败；失败集中在既有
实库配置和 TODO 测试，因此不把 vendor 全量测试记为通过。

## 未执行的现场硬门

本记录只证明源码、依赖、构建和离线行为。球、SSCL、多面体和 dabacon YOFF Snout 的现场样本
尚未齐全，因此未执行维护窗口、双库原子重建、T046–T049 RVM 和最终现场发布；阈值未放宽。
