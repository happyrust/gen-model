//! ReplaySafe 语句规范 + journal validator（ADR-017 §4 / 开发方案 T0.5）。
//!
//! 语句日志不是 redo log：写回是把**语句文本**按原序对持久层重放，且写回失败后
//! 会拿同一份 journal 整体重试。要让「重放 = 直写」且「重试收敛」，进日志的语句
//! 必须满足下列规范（机械可判的部分由 [`validate_statement`] 在 `execute()` 入口
//! 强制；「不以执行时刻的全库查询结果选择写入目标」的完整判定属于 T2.5 的读写集
//! 分类审计，不在文本层）：
//!
//! - **R1 目标显式固定**：`CREATE` / `UPSERT` 的直接目标必须是显式 record id
//!   （`table:id`、`type::thing(..)`、或提前算好的 `$变量`）；`INSERT` /
//!   `INSERT RELATION` 的载荷必须带显式 `id` 键。裸表目标要么随机发号（重放产
//!   生新行）、要么全表写（目标由执行时刻的表内容决定），一律拒绝。
//! - **R2 不依赖随机值**：禁止 `rand` 族（含 `rand::uuid` 等）。
//! - **R3 时钟只进信息性字段**：`time::now()` 只允许出现在赋值位置（`= time::now()`
//!   / `: time::now()`），且不得出现在 `WHERE` 之后——参与目标选择或 id 构造的
//!   时钟在重放时会选出不同的行。
//! - **R4 禁止相对更新**：`+=` / `-=` 重试重放不收敛（chunk 部分提交后整份
//!   journal 重试会二次累加）。写绝对终态。
//!
//! 与 F2（`docs/2026-08-05_fork-surreal-compat-findings.md`）呼应：本仓历史上
//! `define_common_functions` 静默吞语句错误，validator 与执行器不得继承——凡进
//! 暂存或进日志的语句必须 `check()`。

use anyhow::bail;

/// 校验一段将进入语句日志的 SQL（可含多条语句，`;` 分隔）。
///
/// 拒绝即整段拒绝：不合规语句不进暂存、不进日志——在源头挡住，比写回时炸掉
/// 或静默漂移便宜得多。分号粗切在字符串字面量里含 `;` 时会误切，本仓渲染器
/// 不产这类文本；真遇到会在开发期立刻暴露（误报，而不是漏报）。
pub fn validate_statement(sql: &str) -> anyhow::Result<()> {
    for (index, statement) in sql.split(';').enumerate() {
        let statement = strip_comments(statement);
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            continue;
        }
        validate_single(trimmed).map_err(|error| {
            anyhow::anyhow!("journal 语句第 {} 段不满足 ReplaySafe：{error}\n语句：{trimmed}", index + 1)
        })?;
    }
    Ok(())
}

/// 去掉 `--` 行注释（SurrealQL 的 `//` 注释本仓渲染器不产出，不处理）。
fn strip_comments(statement: &str) -> String {
    statement
        .lines()
        .map(|line| match line.find("--") {
            Some(pos) => &line[..pos],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn validate_single(statement: &str) -> anyhow::Result<()> {
    let lowered = statement.to_lowercase();

    // R2：随机值。按 token 边界找，避免误伤 operand/brand 之类的普通词。
    if contains_word(&lowered, "rand") {
        bail!("依赖随机值（rand 族）——重放会产生不同结果");
    }

    // R4：相对更新。
    if lowered.contains("+=") || lowered.contains("-=") {
        bail!("相对更新（+=/-=）重试重放不收敛，必须写绝对终态");
    }

    // R3：time::now() 的位置。
    if let Some(reason) = clock_misuse(&lowered) {
        bail!("{reason}");
    }

    // R1：写语句的目标显式固定。
    let first_word = lowered.split_whitespace().next().unwrap_or_default();
    match first_word {
        "create" | "upsert" => {
            let target = lowered
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .trim_end_matches(|c| c == ';' || c == ',');
            let explicit = target.contains(':')          // table:id（含 ⟨⟩ 与数组 id）
                || target.starts_with("type::thing")     // type::thing('t', …)
                || target.starts_with('$');              // 提前算好的变量
            if !explicit {
                bail!("{first_word} 的目标 `{target}` 不是显式 record id——裸表目标要么随机发号要么全表写");
            }
        }
        "insert" => {
            // INSERT [RELATION] INTO tbl <载荷>：载荷必须带显式 id 键。
            if !lowered.contains("id:") && !lowered.contains("\"id\"") && !lowered.contains("'id'")
            {
                bail!("INSERT 载荷缺显式 id 键——引擎随机发号，重放产生新行");
            }
        }
        "relate" => {
            // RELATE 的边 id 由引擎随机发号且无法显式指定——重放会造出 id 不同的
            // 新边。边写入一律改走带显式 id 的 INSERT RELATION。
            bail!("RELATE 的边 id 随机发号——请改用带显式 id 的 INSERT RELATION");
        }
        // UPDATE / DELETE / RELATE / LET / RETURN / DEFINE 等：目标可由 WHERE 或
        // 变量决定，确定性由 R2/R3 保证，幂等性由 R4 与语义（绝对写）保证。
        _ => {}
    }

    Ok(())
}

/// `time::now` 的每次出现都必须是赋值位置（`=` / `:` 之后），且不得在 WHERE 里。
fn clock_misuse(lowered: &str) -> Option<String> {
    let where_pos = lowered.find(" where ");
    for (pos, _) in lowered.match_indices("time::now") {
        if let Some(wp) = where_pos {
            if pos > wp {
                return Some("time::now() 出现在 WHERE 中——目标选择依赖执行时刻".into());
            }
        }
        let prefix = &lowered[..pos];
        match prefix.trim_end().chars().last() {
            Some('=') | Some(':') => {}
            _ => {
                return Some(
                    "time::now() 不在赋值位置——只允许写进信息性字段（`= time::now()` / `: time::now()`）"
                        .into(),
                );
            }
        }
    }
    None
}

/// `needle` 是否以独立 token 出现（前后都不是标识符字符）。
fn contains_word(haystack: &str, needle: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let abs = start + pos;
        let before_ok = abs == 0
            || !haystack[..abs]
                .chars()
                .last()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after = abs + needle.len();
        let after_ok = after >= haystack.len()
            || !haystack[after..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if before_ok && after_ok {
            return true;
        }
        start = abs + needle.len();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::validate_statement;

    #[test]
    fn accepts_the_persist_path_statement_shapes() {
        // 解析窗口 / 生成产物的常规形态：显式 id 的 UPSERT / DELETE / INSERT RELATION。
        for sql in [
            "UPSERT pe:⟨4000000001_10⟩ CONTENT { noun: 'PIPE', dbnum: 7997 }",
            "UPSERT type::thing('pe', $refno) SET noun = 'PIPE'",
            "DELETE inst_relate WHERE zone_refno = pe:⟨1_2⟩",
            "UPDATE pe:a SET deleted = true",
            "INSERT RELATION INTO pe_owner [{ id: pe_owner:[pe:a, 0], in: pe:a, out: pe:b }]",
            "LET $pe = pe:⟨1_2⟩; UPDATE type::thing('datacenter_version', $pe) SET status = 'Delete'",
            // 信息性时间戳：赋值位置的 time::now() 合法。
            "UPSERT dbnum_watermark:7997 SET applied_sesno = 42, updated_at = time::now()",
            "UPSERT log:x CONTENT { at: time::now(), msg: 'ok' }",
        ] {
            validate_statement(sql).unwrap_or_else(|e| panic!("应接受：{sql}\n{e}"));
        }
    }

    #[test]
    fn rejects_random_values() {
        for sql in [
            "CREATE pe:a SET token = rand()",
            "UPSERT pe:a SET u = rand::uuid()",
            "UPSERT type::thing('t', rand::string(8)) SET x = 1",
        ] {
            assert!(validate_statement(sql).is_err(), "应拒绝：{sql}");
        }
        // 普通词里含 rand 不误伤。
        validate_statement("UPSERT pe:a SET brand = 'acme', operand = 2").expect("不应误伤");
    }

    #[test]
    fn rejects_bare_table_targets_and_id_less_inserts() {
        for sql in [
            "CREATE pe SET noun = 'PIPE'",           // 随机发号
            "UPSERT inst_relate SET dirty = true",   // 全表写
            "INSERT INTO plain_t { v: 1 }",          // 载荷无 id
            "INSERT RELATION INTO rel_t [{ in: pe:a, out: pe:b }]",
            // RELATE 的边 id 无法显式指定，重放必然造新边。
            "RELATE pe:a->room_relate->pe:b SET room_num = 'R101'",
        ] {
            assert!(validate_statement(sql).is_err(), "应拒绝：{sql}");
        }
    }

    #[test]
    fn rejects_clock_outside_informational_assignments() {
        for sql in [
            "DELETE ses WHERE date < time::now()",
            "UPSERT type::thing('t', time::now()) SET x = 1",
        ] {
            assert!(validate_statement(sql).is_err(), "应拒绝：{sql}");
        }
    }

    #[test]
    fn rejects_relative_updates() {
        assert!(validate_statement("UPDATE pe:a SET n += 1").is_err());
        assert!(validate_statement("UPDATE pe:a SET n -= 1").is_err());
    }

    #[test]
    fn multi_statement_scripts_report_the_offending_segment() {
        let error = validate_statement(
            "UPSERT pe:a SET x = 1;\nCREATE pe SET y = 2;\nUPSERT pe:b SET z = 3",
        )
        .expect_err("中段违规应被拒绝");
        assert!(error.to_string().contains("第 2 段"), "{error}");
    }
}
