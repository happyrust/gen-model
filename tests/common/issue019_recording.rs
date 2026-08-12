//! issue-019 的真实删除序列改写成 `aios-session-fixture-v1` 录制单。
//!
//! 由 `db_session_fixture_selfcheck`（阶段一 pack 往返）与 `db8000_session_pairs`
//! （阶段三回归）共用：两边都需要「在没有真实录制之前，先从 issue-019 的 final
//! 造一份合规夹具」。抽成一份的理由很直接——它同时是 pack 的输入契约样例和回归
//! 的数据来源，两处各抄一份必然漂移。
//!
//! 内容：sesno 25 删子件（BOX），26 删父件（EQUI），两个案例都没有 restore 腿。
//! 推导出的台账应为 {24, 25, 26}，final = 26。
//!
//! 不是 `tests/*.rs` 顶层文件，所以不会被当成独立测试目标；两边用 `#[path]` 引。

/// 录制单 JSON 文本（`pipeline::pack` 的输入）。
pub fn issue019_recording() -> String {
    serde_json::json!({
        "dbnum": 8000,
        "baseline_sesno": 24,
        "cases": [
            {
                "id": "child-delete",
                "apply_sesno": 25,
                "refs": { "target": "24384/24779", "owner": "24384/24778" },
                "elements": [
                    { "refno": "24384/24779", "noun": "BOX",
                      "before_apply": true, "after_apply": false }
                ],
                "expected": { "net_window": [ { "refno": "24384/24779", "net": "deleted" } ] }
            },
            {
                "id": "parent-delete",
                "apply_sesno": 26,
                "refs": { "target": "24384/24778", "owner": "24384/24775" },
                "elements": [
                    { "refno": "24384/24778", "noun": "EQUI",
                      "before_apply": true, "after_apply": false }
                ],
                "expected": { "net_window": [ { "refno": "24384/24778", "net": "deleted" } ] }
            }
        ]
    })
    .to_string()
}

/// issue-019 夹具目录（两边的默认数据源）。
pub fn issue019_fixture_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/issues/issue-019-cross-session-parent-child-delete")
}
