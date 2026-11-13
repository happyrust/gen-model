//! 刻度条、迷你折线、详情走势图。用 GPUI 的 `canvas` 自己画：gpui-kit 的 `LineChart` 要求数据从左边
//! 连续排，画不出「当时没有进程」的缺口，也不能让刚开始攒的历史靠右（skill 的诚实规则要求这两样）。
use gpui_kit::*;

use crate::state::RowKind;
use crate::ui::tokens::Tokens;

/// 一行刻度：每格一段时间里最差的状态；没有数据的格画成空心。
pub fn ticks(buckets: Vec<Option<RowKind>>, t: &Tokens) -> impl IntoElement {
    let colors: Vec<Option<Hsla>> = buckets.iter().map(|b| b.map(|k| t.tick(k))).collect();
    let hollow = t.line;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let n = colors.len().max(1) as f32;
            let gap = 2.0f32;
            let w = ((bounds.size.width.as_f32() - gap * (n - 1.0)) / n).max(1.0);
            let h = bounds.size.height;
            for (i, c) in colors.iter().enumerate() {
                let x = bounds.origin.x + px(i as f32 * (w + gap));
                let b = Bounds::new(point(x, bounds.origin.y), size(px(w), h));
                match c {
                    Some(color) => window.paint_quad(fill(b, *color).corner_radii(px(1.5))),
                    None => window.paint_quad(outline(b, hollow, BorderStyle::Solid).corner_radii(px(1.5))),
                }
            }
        },
    )
    .size_full()
}

/// 迷你折线：值按时间排，None 处断开；没有坐标轴。`y_max` 给了就用固定上界，否则按数据。
/// `refs` 是参考线（值, 颜色），画成细虚线。
pub fn spark(values: Vec<Option<f64>>, color: Hsla, y_max: Option<f64>, refs: Vec<(f64, Hsla)>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            paint_series(window, bounds, &values, color, y_max, &refs, 1.5);
        },
    )
    .size_full()
}

fn scale_max(values: &[Option<f64>], y_max: Option<f64>, refs: &[(f64, Hsla)]) -> f64 {
    if let Some(m) = y_max {
        return m.max(1e-6);
    }
    let data_max = values.iter().flatten().fold(0.0f64, |a, b| a.max(*b));
    let ref_max = refs.iter().map(|r| r.0).fold(0.0f64, f64::max);
    (data_max.max(ref_max) * 1.15).max(1e-6)
}

fn paint_series(window: &mut Window, bounds: Bounds<Pixels>, values: &[Option<f64>], color: Hsla, y_max: Option<f64>, refs: &[(f64, Hsla)], width: f32) {
    let n = values.len();
    // 一个点都没有就什么都不画（连参考线也不画，免得没有数据时看着像有一条线）
    if n < 2 || values.iter().all(|v| v.is_none()) {
        return;
    }
    let max = scale_max(values, y_max, refs);
    let w = bounds.size.width.as_f32();
    let h = bounds.size.height.as_f32();
    let pad = 2.0f32;
    let x_of = |i: usize| bounds.origin.x + px(i as f32 / (n - 1) as f32 * w);
    let y_of = |v: f64| bounds.origin.y + px(pad + (h - 2.0 * pad) * (1.0 - (v / max).clamp(0.0, 1.0) as f32));
    for (v, c) in refs {
        if *v <= 0.0 || *v > max {
            continue;
        }
        let mut b = PathBuilder::stroke(px(1.)).dash_array(&[px(3.), px(3.)]);
        b.move_to(point(bounds.origin.x, y_of(*v)));
        b.line_to(point(bounds.origin.x + px(w), y_of(*v)));
        if let Ok(path) = b.build() {
            window.paint_path(path, *c);
        }
    }
    let mut i = 0;
    while i < n {
        if values[i].is_none() {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && values[i].is_some() {
            i += 1;
        }
        let seg = &values[start..i];
        if seg.len() == 1 {
            let v = seg[0].unwrap_or(0.0);
            let dot = Bounds::new(point(x_of(start) - px(1.5), y_of(v) - px(1.5)), size(px(3.), px(3.)));
            window.paint_quad(fill(dot, color).corner_radii(px(1.5)));
            continue;
        }
        let mut b = PathBuilder::stroke(px(width));
        for (k, v) in seg.iter().enumerate() {
            let p = point(x_of(start + k), y_of(v.unwrap_or(0.0)));
            if k == 0 {
                b.move_to(p);
            } else {
                b.line_to(p);
            }
        }
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    }
    // 最后一点加一个小圆点：当前值
    if let Some(Some(v)) = values.last() {
        let dot = Bounds::new(point(x_of(n - 1) - px(2.), y_of(*v) - px(2.)), size(px(4.), px(4.)));
        window.paint_quad(fill(dot, color).corner_radii(px(2.)));
    }
}

/// 详情抽屉里的大走势：同一个 15 分钟窗口，左边写上界和 0，下边写「15 分钟前」「现在」。
pub fn trend(values: Vec<Option<f64>>, color: Hsla, y_max: Option<f64>, refs: Vec<(f64, Hsla)>, top_label: String, t: &Tokens) -> impl IntoElement {
    let grid = t.line;
    let line_refs = refs.clone();
    let chart = canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            for frac in [0.0f32, 0.5, 1.0] {
                let y = bounds.origin.y + px(2.0 + (bounds.size.height.as_f32() - 4.0) * frac);
                window.paint_quad(fill(Bounds::new(point(bounds.origin.x, y), size(bounds.size.width, px(1.))), grid.opacity(0.6)));
            }
            paint_series(window, bounds, &values, color, y_max, &line_refs, 1.8);
        },
    )
    .size_full();
    let muted = t.muted;
    gpui_kit::component::v_flex()
        .w_full()
        .gap_1()
        .child(
            gpui_kit::component::h_flex()
                .w_full()
                .gap_2()
                .child(
                    gpui_kit::component::v_flex()
                        .w_12()
                        .h(px(96.))
                        .justify_between()
                        .text_xs()
                        .text_color(muted)
                        .child(top_label)
                        .child("0"),
                )
                .child(div().flex_1().h(px(96.)).child(chart)),
        )
        .child(
            gpui_kit::component::h_flex()
                .w_full()
                .pl(px(56.))
                .justify_between()
                .text_xs()
                .text_color(muted)
                .child("15 分钟前")
                .child("现在"),
        )
}
