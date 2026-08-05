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
//! - **R3 不依赖时钟**：journal 禁止 `time::now()`；信息性时间戳只属于不重放的
//!   commit tail。这样不需要猜测函数位于赋值还是目标表达式。
//! - **R4 禁止相对更新**：`+=` / `-=` 重试重放不收敛（chunk 部分提交后整份
//!   journal 重试会二次累加）。写绝对终态。
//!
//! 与 F2（`docs/2026-08-05_fork-surreal-compat-findings.md`）呼应：本仓历史上
//! `define_common_functions` 静默吞语句错误，validator 与执行器不得继承——凡进
//! 暂存或进日志的语句必须 `check()`。

use anyhow::bail;
use serde_json::Value as JsonValue;
use surrealdb::sql::{Data, Id, Operator, Statement, Value, Values};

/// 校验一段将进入语句日志的 SQL（可含多条语句）。
///
/// 拒绝即整段拒绝：不合规语句不进暂存、不进日志。解析、注释和字符串边界全部
/// 交给与执行端相同的 SurrealQL parser，validator 不再维护第二套 lexer。
pub fn validate_statement(sql: &str) -> anyhow::Result<()> {
    let query = surrealdb::sql::parse(sql)
        .map_err(|error| anyhow::anyhow!("journal SurrealQL 解析失败：{error}"))?;
    let statements = query.iter().collect::<Vec<_>>();
    let transaction = matches!(statements.first(), Some(Statement::Begin(_)));
    if transaction && !matches!(statements.last(), Some(Statement::Commit(_))) {
        bail!("journal 显式事务必须以 COMMIT 收口");
    }
    for (index, statement) in statements.iter().enumerate() {
        if matches!(statement, Statement::Begin(_) | Statement::Commit(_)) {
            let is_boundary = transaction && (index == 0 || index + 1 == statements.len());
            if is_boundary {
                continue;
            }
            bail!("journal 只接受包住整段脚本的一对 BEGIN/COMMIT");
        }
        if matches!(statement, Statement::Cancel(_)) {
            bail!("journal 不接受 CANCEL TRANSACTION");
        }
        validate_single(statement).map_err(|error| {
            anyhow::anyhow!(
                "journal 语句第 {} 段不满足 ReplaySafe：{error}\n语句：{statement}",
                index + 1
            )
        })?;
    }
    Ok(())
}

pub(crate) fn is_explicit_transaction(sql: &str) -> bool {
    surrealdb::sql::parse(sql).is_ok_and(|query| {
        matches!(query.iter().next(), Some(Statement::Begin(_)))
            && matches!(query.iter().last(), Some(Statement::Commit(_)))
    })
}

/// 已审计的生成级联删除形态：完整事务，顶层只含 LET、DELETE 与条件删除块。
pub(crate) fn validate_scoped_delete_transaction(sql: &str) -> anyhow::Result<()> {
    let query = surrealdb::sql::parse(sql)
        .map_err(|error| anyhow::anyhow!("级联删除 SurrealQL 解析失败：{error}"))?;
    let statements = query.iter().collect::<Vec<_>>();
    if !matches!(statements.first(), Some(Statement::Begin(_)))
        || !matches!(statements.last(), Some(Statement::Commit(_)))
    {
        bail!("级联删除必须由一对 BEGIN/COMMIT 包住");
    }
    for statement in &statements[1..statements.len() - 1] {
        reject_nondeterministic_functions(statement)?;
        if !matches!(statement, Statement::Set(_) | Statement::Delete(_) | Statement::Ifelse(_)) {
            bail!("级联删除事务只允许 LET、DELETE 与 IF 块");
        }
    }
    Ok(())
}

/// 资源门禁用的逻辑写行数估算。显式多行 INSERT 精确计数；带 WHERE 的集合写
/// 只能在执行后知道命中量，先按一条逻辑写计。
pub fn estimate_write_rows(sql: &str) -> anyhow::Result<u64> {
    let query = surrealdb::sql::parse(sql)
        .map_err(|error| anyhow::anyhow!("资源估算 SurrealQL 解析失败：{error}"))?;
    Ok(query
        .iter()
        .map(|statement| match statement {
            Statement::Create(write) => write.what.len() as u64,
            Statement::Upsert(write) => write.what.len() as u64,
            Statement::Update(write) => write.what.len().max(1) as u64,
            Statement::Delete(write) => write.what.len().max(1) as u64,
            Statement::Insert(write) => match &write.data {
                Data::ValuesExpression(rows) => rows.len() as u64,
                Data::SingleExpression(Value::Array(rows)) => rows.len() as u64,
                _ => 1,
            },
            // ponytail: WHERE 集合写按 1 行代理；若实测资源门禁低报，再接执行响应计数。
            _ => 0,
        })
        .sum())
}

fn validate_single(statement: &Statement) -> anyhow::Result<()> {
    reject_nondeterministic_functions(statement)?;

    match statement {
        Statement::Create(write) => {
            require_explicit_target("CREATE", &write.what)?;
            validate_data(write.data.as_ref())?;
        }
        Statement::Upsert(write) => {
            require_explicit_target("UPSERT", &write.what)?;
            validate_data(write.data.as_ref())?;
        }
        Statement::Update(write) => {
            require_bounded_target("UPDATE", &write.what, write.cond.is_some())?;
            validate_data(write.data.as_ref())?;
        }
        Statement::Delete(write) => {
            require_bounded_target("DELETE", &write.what, write.cond.is_some())?;
        }
        Statement::Insert(write) => {
            require_insert_ids(&write.data)?;
            validate_data(Some(&write.data))?;
        }
        Statement::Set(_) => {}
        Statement::Relate(_) => {
            bail!("RELATE 的边 id 随机发号——请改用带显式 id 的 INSERT RELATION")
        }
        _ => bail!("journal 不接受此语句类型（只允许有界写与 LET）"),
    }
    Ok(())
}

fn validate_data(data: Option<&Data>) -> anyhow::Result<()> {
    let operators = match data {
        Some(Data::SetExpression(items) | Data::UpdateExpression(items)) => items,
        _ => return Ok(()),
    };
    if operators
        .iter()
        .any(|(_, operator, _)| matches!(operator, Operator::Inc | Operator::Dec | Operator::Ext))
    {
        bail!("相对更新（+=/-=/+?=）重试重放不收敛，必须写绝对终态");
    }
    Ok(())
}

fn require_explicit_target(kind: &str, values: &Values) -> anyhow::Result<()> {
    if values.len() != 1 || !is_explicit_record(&values[0]) {
        bail!("{kind} 目标必须是单个显式 record id");
    }
    Ok(())
}

fn require_bounded_target(kind: &str, values: &Values, has_where: bool) -> anyhow::Result<()> {
    if values.iter().all(is_bounded_target)
        || (has_where && values.iter().all(|value| matches!(value, Value::Table(_))))
    {
        return Ok(());
    }
    bail!("{kind} 目标必须是显式 record id，或带 WHERE 的单表目标")
}

fn is_bounded_target(value: &Value) -> bool {
    is_explicit_record(value)
        || matches!(
            value,
            Value::Edges(edges)
                if !matches!(&edges.from.id, Id::Generate(_) | Id::Range(_))
                    && !edges.what.is_empty()
        )
}

fn is_explicit_record(value: &Value) -> bool {
    match value {
        Value::Thing(thing) => !matches!(&thing.id, Id::Generate(_) | Id::Range(_)),
        Value::Param(_) => true,
        Value::Function(function) => function.name() == Some("type::thing"),
        _ => false,
    }
}

fn require_insert_ids(data: &Data) -> anyhow::Result<()> {
    let has_id = |fields: &Vec<(surrealdb::sql::Idiom, Value)>| {
        fields.iter().any(|(field, _)| field.to_string() == "id")
    };
    let valid = match data {
        Data::ValuesExpression(rows) => !rows.is_empty() && rows.iter().all(has_id),
        Data::SingleExpression(Value::Object(object)) => object.contains_key("id"),
        Data::SingleExpression(Value::Array(array)) => !array.is_empty()
            && array.iter().all(|value| {
                matches!(value, Value::Object(object) if object.contains_key("id"))
            }),
        _ => false,
    };
    if !valid {
        bail!("INSERT 每行载荷都必须带显式 id 键");
    }
    Ok(())
}

fn reject_nondeterministic_functions(statement: &Statement) -> anyhow::Result<()> {
    let ast = serde_json::to_value(statement)?;
    visit_ast(&ast)
}

fn visit_ast(node: &JsonValue) -> anyhow::Result<()> {
    match node {
        JsonValue::Object(object) => {
            if let Some(function) = object.get("Function") {
                let parts = function
                    .get("Normal")
                    .or_else(|| function.get("Custom"))
                    .and_then(JsonValue::as_array)
                    .ok_or_else(|| anyhow::anyhow!("journal 不接受脚本或匿名函数"))?;
                let name = parts
                    .first()
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| anyhow::anyhow!("journal 不接受未知函数 AST"))?;
                if name == "time::now" || name == "rand" || name.starts_with("rand::") {
                    bail!("journal 依赖非确定函数 `{name}`");
                }
            }
            for value in object.values() {
                visit_ast(value)?;
            }
        }
        JsonValue::Array(values) => {
            for value in values {
                visit_ast(value)?;
            }
        }
        _ => {}
    }
    Ok(())
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
            "DELETE pe:⟨1_2⟩->ref_rev",
            "UPDATE pe:a SET deleted = true",
            "INSERT RELATION INTO pe_owner [{ id: pe_owner:[pe:a, 0], in: pe:a, out: pe:b }]",
            "LET $pe = pe:⟨1_2⟩; UPDATE type::thing('datacenter_version', $pe) SET status = 'Delete'",
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
    fn rejects_clock_anywhere_in_journal() {
        for sql in [
            "DELETE ses WHERE date < time::now()",
            "UPSERT type::thing('t', time::now()) SET x = 1",
            "UPSERT log:x CONTENT { at: time::now(), msg: 'ok' }",
        ] {
            assert!(validate_statement(sql).is_err(), "应拒绝：{sql}");
        }
    }

    #[test]
    fn rejects_relative_updates() {
        assert!(validate_statement("UPDATE pe:a SET n += 1").is_err());
        assert!(validate_statement("UPDATE pe:a SET n -= 1").is_err());
        assert!(
            validate_statement("UPDATE pe:a SET msg = '--', n += 1").is_err(),
            "字符串里的注释符不能截断后续 AST"
        );
        validate_statement("UPSERT pe:a SET msg = 'a;b -- literal'")
            .expect("字符串里的分号和注释符必须由 parser 正确处理");
    }

    #[test]
    fn multi_statement_scripts_report_the_offending_segment() {
        let error = validate_statement(
            "UPSERT pe:a SET x = 1;\nCREATE pe SET y = 2;\nUPSERT pe:b SET z = 3",
        )
        .expect_err("中段违规应被拒绝");
        assert!(error.to_string().contains("第 2 段"), "{error}");
    }

    #[test]
    fn accepts_only_complete_outer_transactions() {
        validate_statement(
            "BEGIN TRANSACTION; DELETE pe:a->ref_rev; UPSERT pe:a SET noun = 'PIPE'; COMMIT TRANSACTION;",
        )
        .expect("完整外层事务可作为一个原子 journal 单元");
        assert!(validate_statement("BEGIN TRANSACTION; UPSERT pe:a SET x = 1").is_err());
        assert!(validate_statement("BEGIN; UPSERT pe:a SET x = 1; CANCEL; COMMIT").is_err());
    }
}
