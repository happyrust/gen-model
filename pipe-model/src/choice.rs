//! CHOICE 答卷串编解码（功能文档 §18.8）。
//!
//! E3D 词法（componenttable.pmlobj :380-400 权威解析 + specborechange 两处用法）：
//! - 整串 = `SPEC /name` 后跟问答词对
//! - `PBOR 0100`：答案词**首字符是孔位号**，余下为值 → 规范键 `PBOR0`、值 `100`
//! - `PSHA` 同理（无数字首字符补 0）；`PWID`/`PHEI` 后缀跟随最近 boreID
//! - `STYP` 等裸键不带孔位后缀
//!
//! 命令侧两种等价写法，内核都收：
//! - CREATE：`SELECT WITH SPEC /x PBOR 0 100`（孔位分词）
//! - RESELECT：`RESELECT WITH SPEC /x PBOR0 100`（键带号）

/// 规范化键值对：问键恒为规范形（`PBOR0`/`PBOR1`/`STYP`…）。
pub type QuestionAnswers = Vec<(String, String)>;

/// 渲染为 CHOICE 命令尾串，RESELECT 形态（`PBOR0 100` 键带号）。
pub fn render_choice(spec_name: &str, qa: &QuestionAnswers) -> String {
    let mut s = format!("SPEC /{spec_name}");
    for (q, a) in qa {
        s.push(' ');
        s.push_str(q);
        s.push(' ');
        s.push_str(a);
    }
    s
}

/// 渲染为 CREATE 形态：`PBOR 0 100`（孔位号拆到值词前）。
///
/// 非 PBOR 键原样成对输出。
pub fn render_choice_for_create(spec_name: &str, qa: &QuestionAnswers) -> String {
    let mut s = format!("SPEC /{spec_name}");
    for (q, a) in qa {
        s.push(' ');
        if let Some(idx) = q.strip_prefix("PBOR") {
            if !idx.is_empty() && idx.chars().all(|c| c.is_ascii_digit()) {
                s.push_str(&format!("PBOR {idx} {a}"));
                continue;
            }
        }
        s.push_str(q);
        s.push(' ');
        s.push_str(a);
    }
    s
}

/// 解析 E3D 原生 CHOICE 串 → (规格名, 规范键值对)。
///
/// 对 `PBOR 0100` 胶合形做拆键；`PBOR 0 100`（CREATE 三分）也兼容——
/// 当 PBOR 后紧跟两个词且首词是单位数孔位号时按三分形合并。
/// 返回 `Err` 当词数无法成对。
pub fn parse_choice(input: &str) -> Result<(String, QuestionAnswers), String> {
    let toks: Vec<&str> = input.split_whitespace().collect();
    if toks.len() < 2 || !toks[0].eq_ignore_ascii_case("SPEC") {
        return Err(format!("CHOICE 串缺 SPEC /name 头: {input:?}"));
    }
    let spec = toks[1].trim_start_matches('/').to_string();
    let mut qa = QuestionAnswers::new();
    let mut last_bore = String::from("0");
    let mut i = 2;
    while i < toks.len() {
        let q = toks[i];
        let a = toks.get(i + 1).copied().unwrap_or("");
        // CREATE 三分形：`PBOR <digit> <value>`。
        if q.eq_ignore_ascii_case("PBOR")
            && a.len() == 1
            && a.chars().all(|c| c.is_ascii_digit())
            && toks.get(i + 2).is_some()
        {
            last_bore = a.to_string();
            qa.push((format!("PBOR{a}"), toks[i + 2].to_string()));
            i += 3;
            continue;
        }
        // 胶合形：`PBOR <idx><value>` / `PSHA <idx><value>`。
        if q.eq_ignore_ascii_case("PBOR") || q.eq_ignore_ascii_case("PSHA") {
            let (idx, val) = if let Some(first) = a.chars().next().filter(|c| c.is_ascii_digit()) {
                last_bore = first.to_string();
                (first.to_string(), a[first.len_utf8()..].to_string())
            } else {
                ("0".into(), a.to_string())
            };
            qa.push((format!("{}{idx}", q.to_ascii_uppercase()), val));
            i += 2;
            continue;
        }
        // PWID/PHEI 跟随最近孔位号；其余裸键。
        let key = if q.eq_ignore_ascii_case("PWID") || q.eq_ignore_ascii_case("PHEI") {
            format!("{}{last_bore}", q.to_ascii_uppercase())
        } else {
            q.to_ascii_uppercase()
        };
        qa.push((key, a.to_string()));
        i += 2;
    }
    Ok((spec, qa))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_glued_pbor() {
        // `SPEC /A1B PBOR 0100 STYP EQUAL` → PBOR0=100（首字符 0 是孔位号）。
        let (spec, qa) = parse_choice("SPEC /A1B PBOR 0100 STYP EQUAL").unwrap();
        assert_eq!(spec, "A1B");
        assert_eq!(
            qa,
            vec![
                ("PBOR0".to_string(), "100".to_string()),
                ("STYP".to_string(), "EQUAL".to_string()),
            ]
        );
    }

    #[test]
    fn parse_create_three_token_form() {
        // CREATE 形 `PBOR 0 100` 与胶合形 `PBOR 0100` 归一到同一键值。
        let (_, qa) = parse_choice("SPEC /A1B PBOR 0 100").unwrap();
        assert_eq!(qa, vec![("PBOR0".to_string(), "100".to_string())]);
    }

    #[test]
    fn roundtrip_reselect_form() {
        let qa = vec![
            ("PBOR0".to_string(), "100".to_string()),
            ("STYP".to_string(), "EQUAL".to_string()),
        ];
        let s = render_choice("A1B", &qa);
        assert_eq!(s, "SPEC /A1B PBOR0 100 STYP EQUAL");
    }

    #[test]
    fn roundtrip_create_form() {
        let qa = vec![("PBOR1".to_string(), "80".to_string())];
        assert_eq!(render_choice_for_create("A1B", &qa), "SPEC /A1B PBOR 1 80");
    }
}
