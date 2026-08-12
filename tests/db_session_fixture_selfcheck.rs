//! 阶段一验收（docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md §1）：
//! 通用切割模块对既有 issue-019 夹具做重放自检。
//!
//! 验收标准原文：「对现存 issue-019 zip 里的 final 文件运行新工具，能切出
//! sesno 24/25 快照且 SHA256 与 manifest 记录一致」。issue-019 的三份快照是
//! 录制时从**源文件**切的（专用实现），本测试从 zip 里的 **final（sesno 26）**
//! 用通用模块现切同样的 sesno——两条路径产出逐字节相同，才证明
//! 「任意历史可从最终文件精确还原」对真实 db8000 会话链成立。
//!
//! 离线、零外部依赖，与 CI 现跑的 `db8000_two_delete_fixture` 用同一份夹具：
//! `cargo test --test db_session_fixture_selfcheck -- --nocapture`

#[path = "../src/bin/db8000_two_delete_fixture/archive.rs"]
#[allow(dead_code)]
mod issue019;

#[path = "../src/bin/db_session_fixture/format.rs"]
#[allow(dead_code)]
mod format;

#[path = "../src/bin/db_session_fixture/archive_util.rs"]
#[allow(dead_code)]
mod archive_util;

#[path = "../src/bin/db_session_fixture/session_cut.rs"]
#[allow(dead_code)]
mod session_cut;

// 与 bin 根同名声明，`pipeline` 内部的 `crate::format` 等路径在两个 crate 里
// 都解析得到——测试跑的是 bin 那一份实现本体，不是它的复制品。
#[path = "../src/bin/db_session_fixture/pipeline.rs"]
#[allow(dead_code)]
mod pipeline;

use parse_pdms_db::paged::PagedDbSession;
use std::fs;
use std::path::{Path, PathBuf};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/issues/issue-019-cross-session-parent-child-delete")
}

/// 从 final 现切 24/25/26：字节散列与 issue-019 台账逐一相等（26 顺带证明
/// 「切到头指针会话」等于原文件本身），每一切都过 sesno + 存在性验证闸。
#[test]
fn generic_cutter_replays_issue019_snapshots_byte_for_byte() {
    let fixture = issue019::verify_and_extract(&fixture_root()).expect("verify issue-019 fixture");
    let final_path = fixture
        .path_for_role("parent_deleted")
        .expect("final snapshot path");
    let final_bytes = fs::read(&final_path).expect("read final snapshot");

    let chain = session_cut::session_chain(&final_bytes).expect("walk session chain");
    assert_eq!(
        chain.latest_sesno, fixture.manifest.window.end_sesno,
        "final 头指针必须指向窗口末会话"
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let mut probes_checked = 0usize;
    for spec in &fixture.manifest.snapshots {
        let cut = chain
            .cut_for(spec.sesno)
            .expect("台账 sesno 必须在会话链里");
        let out = temp.path().join(format!("sesno-{:03}", spec.sesno));
        session_cut::write_snapshot(&final_bytes, cut, &out).expect("cut snapshot");

        // 验证闸（方案 §1 第 4 条）：切出的文件自称的 sesno 必须是它自己。
        let mut db = PagedDbSession::open(&out).expect("open cut snapshot");
        assert_eq!(
            db.snapshot().sesno,
            spec.sesno,
            "sesno {} 的切割快照打开后必须坐在自己的会话上",
            spec.sesno
        );
        // 存在性探针沿用 issue-019 manifest 里逐快照声明的元素终态。
        let refnos: Vec<_> = spec
            .elements
            .iter()
            .map(|element| format::parse_refno(&element.refno).expect("manifest refno"))
            .collect();
        let found = db.read_raw_records(&refnos).expect("probe records");
        for element in &spec.elements {
            let refno = format::parse_refno(&element.refno).expect("manifest refno");
            assert_eq!(
                found.contains_key(&refno),
                element.present,
                "sesno {} 上 {}（{}）的存在性与台账不符",
                spec.sesno,
                element.refno,
                element.noun
            );
            probes_checked += 1;
        }

        // 验收本体：现切字节与录制时入库的历史快照逐字相同。
        assert_eq!(
            fs::metadata(&out).expect("stat cut").len(),
            spec.bytes,
            "sesno {} 现切大小必须与台账一致",
            spec.sesno
        );
        assert_eq!(
            archive_util::sha256_file(&out).expect("hash cut"),
            spec.sha256,
            "sesno {} 现切 SHA256 必须与 issue-019 台账一致——历史还原对账失败",
            spec.sesno
        );
    }
    // 反空转：三份快照各带 3 条元素声明，探针必须真的跑过。
    assert!(probes_checked >= 9, "存在性探针只跑了 {probes_checked} 条");
}

/// issue-019 的真实删除序列改写成 `aios-session-fixture-v1` 录制单：
/// 25 删子件（BOX）、26 删父件（EQUI），两个案例都无 restore 腿。
/// 推导出的台账应为 {24, 25, 26}，final = 26。
fn issue019_recording() -> String {
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

/// `pack` 的端到端覆盖：真实源文件 → 夹具目录 → 复验全绿，且台账散列与
/// issue-019 独立录制的那份逐一相等。
///
/// 阶段二是一次性 E3D 录制，pack 出错要再占一个生产空窗重录，所以它必须在
/// 录制之前就有覆盖。用 issue-019 的 final（sesno 26）当源：它本身就是真实
/// db8000 会话链，pack 的每一步（切最终文件、切台账、验证闸、打包、复验）
/// 都跑在真数据上。
#[test]
fn pack_builds_a_verifiable_fixture_matching_the_recorded_ledger() {
    let fixture = issue019::verify_and_extract(&fixture_root()).expect("verify issue-019 fixture");
    let source = fixture
        .path_for_role("parent_deleted")
        .expect("final snapshot path");

    let work = tempfile::tempdir().expect("tempdir");
    let recording = work.path().join("recording.json");
    fs::write(&recording, issue019_recording()).expect("write recording");
    let out = work.path().join("issue-019-repacked");

    let report = pipeline::pack(Some(&source), &recording, &out, Some(8000), false)
        .expect("pack must produce a fixture that passes verification");
    assert_eq!(report.dbnum, 8000);
    assert_eq!(report.final_sesno, 26);
    assert_eq!(report.cases, 2);
    assert_eq!(report.ledger_len, 3, "台账应为 sesno 24/25/26");
    // 探针：24 上子件在、25 上子件不在 + 父件在、26 上父件不在。
    assert_eq!(report.probes_checked, 4, "四条存在性探针必须都跑过");

    // 夹具形状：只入库最终文件的 zip + manifest + SHA256SUMS，中间切割不留痕。
    assert!(out.join("manifest.json").is_file());
    assert!(out.join("SHA256SUMS").is_file());
    assert!(out.join("db8000-sesno24-26.zip").is_file());
    assert!(!out.join("cuts").exists(), "历史切割不得入库");
    assert!(!out.join("final").exists(), "最终文件明文不得入库（只在 zip 里）");

    // 交叉对账：pack 从 final 切出的台账散列，必须与 issue-019 当年从**源文件**
    // 逐个切出、独立入库的那三份逐一相等。两条录制路径产出同一批字节，
    // 「任意历史可从最终文件精确还原」才算被证到。
    let manifest = read_manifest(&out);
    for spec in &manifest.session_snapshots {
        let recorded = fixture
            .manifest
            .snapshots
            .iter()
            .find(|snapshot| snapshot.sesno == spec.sesno)
            .unwrap_or_else(|| panic!("issue-019 台账里没有 sesno {}", spec.sesno));
        assert_eq!(
            spec.sha256, recorded.sha256,
            "sesno {} 的台账散列与 issue-019 独立录制的不符",
            spec.sesno
        );
        assert_eq!(spec.bytes, recorded.bytes, "sesno {} 大小不符", spec.sesno);
    }

    // 防伪：台账被改一个字节，复验必须红——否则「现切对账」这道闸是摆设。
    let mut tampered = manifest;
    let victim = &mut tampered.session_snapshots[0];
    victim.sha256 = flip_last_hex(&victim.sha256);
    fs::write(
        out.join("manifest.json"),
        serde_json::to_vec_pretty(&tampered).expect("serialize tampered manifest"),
    )
    .expect("write tampered manifest");
    let error = pipeline::verify_fixture(&out).expect_err("篡改台账后复验必须失败");
    assert!(
        error.to_string().contains("历史还原对账失败"),
        "失败原因应指向现切对账: {error:#}"
    );
}

/// `inspect` 是录制脚本判「这个宏是不是恰好推进了一个会话」的唯一依据，
/// 所以它报的链必须与切割用的同一份解析一致。
#[test]
fn inspect_reports_the_whole_session_chain() {
    let fixture = issue019::verify_and_extract(&fixture_root()).expect("verify issue-019 fixture");
    let final_path = fixture
        .path_for_role("parent_deleted")
        .expect("final snapshot path");

    let report = pipeline::inspect(&final_path).expect("inspect final snapshot");
    assert_eq!(report.latest_sesno, 26);
    for sesno in [24, 25, 26] {
        assert!(
            report.sesnos.contains(&sesno),
            "会话链应含 sesno {sesno}: {:?}",
            report.sesnos
        );
    }
    assert!(
        report.sesnos.windows(2).all(|pair| pair[0] < pair[1]),
        "sesno 必须升序，录制脚本按相邻差判进度: {:?}",
        report.sesnos
    );

    // 早于窗口的历史快照自然只报到自己那一站。
    let baseline = fixture.path_for_role("baseline").expect("baseline path");
    assert_eq!(
        pipeline::inspect(&baseline)
            .expect("inspect baseline")
            .latest_sesno,
        24
    );
}

fn read_manifest(root: &Path) -> format::SessionFixtureManifest {
    serde_json::from_slice(&fs::read(root.join("manifest.json")).expect("read manifest"))
        .expect("parse manifest")
}

fn flip_last_hex(digest: &str) -> String {
    let (head, last) = digest.split_at(digest.len() - 1);
    let flipped = if last == "0" { "1" } else { "0" };
    format!("{head}{flipped}")
}
