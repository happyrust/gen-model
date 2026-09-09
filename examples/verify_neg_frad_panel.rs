//! 真实数据终验：ZDJ `zdj7150` 里的甲板板 PANE `/DK_11000_TOS_P01` 轮廓带一个
//! FRAD = −25mm 的负倒角顶点。把它的**真实轮廓**喂进 gen-model 生产用的
//! `libgm_discretise::profile_spans`（本轮修好的那条），坐实负 FRAD 顶点被展开成
//! **以该顶点为心、半径 25mm 的凹圆弧**（`mthArcRadius` 口径），而不是旧的尖角。
//!
//! 顺带把「修复前（负 FRAD 当尖角）」与「修复后（凹圆角）」两条 2D 轮廓折线各写一份
//! JSON，给 `scripts/plot_neg_frad_panel.py` 画对照图（截图终验）。
//!
//! ```text
//! cargo run --release --example verify_neg_frad_panel -- \
//!     --attlib E:\reverse\e3d\shadow_e3d31_aps_all\attlib.dat \
//!     --db D:\AVEVA\Projects\E3D3.1\ZDJ\ZDJ000\zdj7150_0001 \
//!     --template E:\reverse\e3d\shadow_e3d31_aps_all --name /DK_11000_TOS_P01
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use aios_database::fast_model::libgm_discretise::{
    FACET_TOL_MM, ProfileSpan, profile_spans, span_arc, span_polyline_by_tol,
};
use anyhow::{Context, bail};
use e3d_io::db_element::{DbFilePin, DbSet, template_file_for};
use e3d_model::profile::loop_profile;

/// 一条环按 span 折成 2D 折线（与挤出 / 板生成同一口径）。
fn outline(spans: &[ProfileSpan]) -> Vec<[f64; 2]> {
    let n = spans.len();
    let mut pts = Vec::new();
    for i in 0..n {
        let a = spans[i].point;
        let b = spans[(i + 1) % n].point;
        let seg = span_polyline_by_tol(a, b, spans[i].bulge, FACET_TOL_MM);
        // 段间端点重复，去掉后一段的首点。
        if i == 0 {
            pts.extend(seg);
        } else {
            pts.extend(seg.into_iter().skip(1));
        }
    }
    pts
}

fn main() -> anyhow::Result<()> {
    let mut attlib: Option<PathBuf> = None;
    let mut db_file: Option<PathBuf> = None;
    let mut template: Option<PathBuf> = None;
    let mut db_type = "DESI".to_string();
    let mut name: Option<String> = None;
    let mut out = PathBuf::from("out/neg-frad-verify");

    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().with_context(|| format!("参数 {flag} 缺值"));
        match flag.as_str() {
            "--attlib" => attlib = Some(PathBuf::from(value()?)),
            "--db" => db_file = Some(PathBuf::from(value()?)),
            "--template" => template = Some(PathBuf::from(value()?)),
            "--db-type" => db_type = value()?,
            "--name" => name = Some(value()?),
            "--out" => out = PathBuf::from(value()?),
            other => bail!("不认识的参数 {other}"),
        }
    }
    let attlib = attlib.context("缺 --attlib")?;
    let db_file = db_file.context("缺 --db")?;
    let template = template.context("缺 --template")?;
    let name = name.context("缺 --name")?;
    let template = if template.is_dir() {
        template_file_for(&template, &db_type)?
    } else {
        template
    };

    let set = Arc::new(DbSet::with_attlib_file(&attlib)?);
    let dbno = set.add_db(DbFilePin {
        file: db_file,
        template,
        db_type: Some(db_type),
        sesno: None,
    })?;

    let found = set.find_named(dbno, &name)?;
    let [refno] = found[..] else {
        bail!("`{name}` 命中 {} 个，要恰好 1 个", found.len());
    };
    let el = set.element(refno);
    println!("元素 {} {}  名 {name}", el.refno(), el.element_type()?);

    let profile = loop_profile(&el)?;
    let ring: Vec<[f64; 3]> = profile.rings[0]
        .iter()
        .map(|v| [v.x, v.y, v.frad])
        .collect();
    println!("外环 {} 个顶点，height={:.3}", ring.len(), profile.height);

    // 真实负 FRAD 顶点。
    let mut neg = Vec::new();
    for (i, v) in ring.iter().enumerate() {
        if v[2] <= -0.1 {
            println!("  顶点[{i}] ({:.3}, {:.3})  FRAD={:.3}", v[0], v[1], v[2]);
            neg.push((i, [v[0], v[1]], v[2]));
        }
    }
    if neg.is_empty() {
        bail!("`{name}` 外环没有负 FRAD 顶点——换一个构件");
    }

    // 修复后：真实轮廓过 gen-model 生产用的 profile_spans。
    let spans_fixed = profile_spans(&ring);
    // 修复前对照：把负 FRAD 顶点强制当尖角（旧行为）。
    let ring_sharp: Vec<[f64; 3]> = ring
        .iter()
        .map(|v| if v[2] <= -0.1 { [v[0], v[1], 0.0] } else { *v })
        .collect();
    let spans_sharp = profile_spans(&ring_sharp);

    // 逐个负 FRAD 顶点：在展开结果里找到那段弧，坐实圆心落在该顶点、半径 = |FRAD|。
    let mut all_ok = true;
    for (_i, [vx, vy], frad) in &neg {
        let want_r = frad.abs();
        let mut matched = None;
        let n = spans_fixed.len();
        for j in 0..n {
            if spans_fixed[j].bulge.abs() < 1e-9 {
                continue;
            }
            let a = spans_fixed[j].point;
            let b = spans_fixed[(j + 1) % n].point;
            if let Some(arc) = span_arc(a, b, spans_fixed[j].bulge) {
                let dc = ((arc.centre[0] - vx).powi(2) + (arc.centre[1] - vy).powi(2)).sqrt();
                if dc < 1.0 && (arc.radius - want_r).abs() < 1.0 {
                    matched = Some((arc.centre, arc.radius));
                    break;
                }
            }
        }
        match matched {
            Some((c, r)) => println!(
                "  ✔ 负 FRAD 顶点 ({vx:.1},{vy:.1}) → 凹弧：圆心 ({:.3},{:.3})≈顶点, 半径 {:.3}≈{want_r}",
                c[0], c[1], r
            ),
            None => {
                all_ok = false;
                println!("  ✗ 顶点 ({vx:.1},{vy:.1}) 没找到半径≈{want_r} 且以它为心的弧");
            }
        }
    }

    std::fs::create_dir_all(&out)?;
    let fixed_pts = outline(&spans_fixed);
    let sharp_pts = outline(&spans_sharp);
    std::fs::write(
        out.join("outline-fixed.json"),
        serde_json::to_vec_pretty(&fixed_pts)?,
    )?;
    std::fs::write(
        out.join("outline-sharp.json"),
        serde_json::to_vec_pretty(&sharp_pts)?,
    )?;
    let neg_xy: Vec<[f64; 2]> = neg.iter().map(|(_, xy, _)| *xy).collect();
    std::fs::write(
        out.join("neg-vertices.json"),
        serde_json::to_vec_pretty(&neg_xy)?,
    )?;
    println!(
        "轮廓折线写到 {}（fixed {} 点 / sharp {} 点）",
        out.display(),
        fixed_pts.len(),
        sharp_pts.len()
    );

    if !all_ok {
        bail!("有负 FRAD 顶点没对上凹弧");
    }
    println!("真实数据终验通过：负 FRAD 顶点全部展开成以顶点为心、半径 |FRAD| 的凹圆弧。");
    Ok(())
}
