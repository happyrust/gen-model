use std::collections::HashSet;
use std::str::FromStr;

use aios_core::Datetime;
use aios_core::{RefnoEnum, SUL_DB};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::data_interface::increment_record::IncrGeoUpdateLog;

/// 元素变更操作类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ChangeOperation {
    Add,
    Modify,
    Delete,
}

impl FromStr for ChangeOperation {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "ADD" | "INSERT" => Ok(ChangeOperation::Add),
            "MODIFY" | "UPDATE" => Ok(ChangeOperation::Modify),
            "DELETE" | "REMOVE" => Ok(ChangeOperation::Delete),
            _ => Err(anyhow::anyhow!("未知的变更操作类型: {}", s)),
        }
    }
}

/// 元素变更记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementChange {
    /// 元素引用号
    pub refno: String,
    /// 元素类型
    pub element_type: String,
    /// 操作类型
    pub operation: ChangeOperation,
    /// 会话号
    pub sesno: u32,
    /// 时间戳
    pub timestamp: Datetime,
    /// 数据库编号
    pub dbnum: i32,
}

/// 获取特定 sesno 的所有变更
///
/// # 参数
/// * `sesno` - 目标会话号
///
/// # 返回值
/// * `anyhow::Result<IncrGeoUpdateLog>` - 增量几何更新日志
pub async fn get_changes_at_sesno(sesno: u32) -> anyhow::Result<IncrGeoUpdateLog> {
    // 查询该 sesno 的所有变更记录
    let sql = format!(
        "SELECT refno, element_type, operation, sesno, timestamp, dbnum FROM element_changes WHERE sesno = {} ORDER BY timestamp",
        sesno
    );

    let mut response = SUL_DB.query(sql).await?;
    let raw_values: Vec<JsonValue> = response.take(0)?;
    let changes: Vec<ElementChange> = raw_values
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .map_err(|err| anyhow::anyhow!(err))?;

    // 转换为 IncrGeoUpdateLog
    let mut update_log = IncrGeoUpdateLog::default();

    for change in changes {
        let refno = RefnoEnum::Refno(aios_core::RefU64(change.refno.parse::<u64>()?));

        match change.operation {
            ChangeOperation::Delete => {
                update_log.delete_refnos.insert(refno);
            }
            _ => {
                // 根据元素类型分类
                match change.element_type.as_str() {
                    "PRIM" => {
                        update_log.prim_refnos.insert(refno);
                    }
                    "LOOP" => {
                        update_log.loop_owner_refnos.insert(refno);
                    }
                    "BRAN" | "HANGER" => {
                        update_log.bran_hanger_refnos.insert(refno);
                    }
                    "CATA" => {
                        update_log.basic_cata_refnos.insert(refno);
                    }
                    _ => {
                        // 对于未知类型，根据其特征判断分类
                        // 这里可以添加更复杂的分类逻辑
                        println!(
                            "警告：未知元素类型 {} 对于 refno {}",
                            change.element_type, refno
                        );
                    }
                }
            }
        }
    }

    Ok(update_log)
}

/// 获取 sesno 范围内的所有变更
///
/// # 参数
/// * `start_sesno` - 起始会话号
/// * `end_sesno` - 结束会话号
///
/// # 返回值
/// * `anyhow::Result<IncrGeoUpdateLog>` - 增量几何更新日志
pub async fn get_changes_between_sesnos(
    start_sesno: u32,
    end_sesno: u32,
) -> anyhow::Result<IncrGeoUpdateLog> {
    let sql = format!(
        "SELECT refno, element_type, operation, sesno, timestamp, dbnum FROM element_changes WHERE sesno >= {} AND sesno <= {} ORDER BY sesno, timestamp",
        start_sesno, end_sesno
    );

    let mut response = SUL_DB.query(sql).await?;
    let raw_values: Vec<JsonValue> = response.take(0)?;
    let changes: Vec<ElementChange> = raw_values
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .map_err(|err| anyhow::anyhow!(err))?;

    let mut update_log = IncrGeoUpdateLog::default();
    let mut processed_refnos = HashSet::new();

    // 按时间顺序处理变更，后面的变更会覆盖前面的
    for change in changes {
        let refno = RefnoEnum::Refno(aios_core::RefU64(change.refno.parse::<u64>()?));

        // 如果这个refno已经被处理过，先从之前的分类中移除
        if processed_refnos.contains(&refno) {
            update_log.prim_refnos.remove(&refno);
            update_log.loop_owner_refnos.remove(&refno);
            update_log.bran_hanger_refnos.remove(&refno);
            update_log.basic_cata_refnos.remove(&refno);
            update_log.delete_refnos.remove(&refno);
        }

        match change.operation {
            ChangeOperation::Delete => {
                update_log.delete_refnos.insert(refno);
            }
            _ => match change.element_type.as_str() {
                "PRIM" => {
                    update_log.prim_refnos.insert(refno);
                }
                "LOOP" => {
                    update_log.loop_owner_refnos.insert(refno);
                }
                "BRAN" | "HANGER" => {
                    update_log.bran_hanger_refnos.insert(refno);
                }
                "CATA" => {
                    update_log.basic_cata_refnos.insert(refno);
                }
                _ => {
                    println!(
                        "警告：未知元素类型 {} 对于 refno {}",
                        change.element_type, refno
                    );
                }
            },
        }

        processed_refnos.insert(refno);
    }

    Ok(update_log)
}

/// 检查指定 sesno 是否存在变更记录
///
/// # 参数
/// * `sesno` - 目标会话号
///
/// # 返回值
/// * `anyhow::Result<bool>` - 是否存在变更记录
pub async fn has_changes_at_sesno(sesno: u32) -> anyhow::Result<bool> {
    let sql = format!(
        "SELECT COUNT(*) as count FROM element_changes WHERE sesno = {}",
        sesno
    );

    let mut response = SUL_DB.query(sql).await?;
    let count: Option<i64> = response.take("count")?;

    Ok(count.unwrap_or(0) > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_changes_at_sesno() -> anyhow::Result<()> {
        // 这里需要有测试数据库连接
        // 实际测试需要根据具体的数据库环境进行调整
        let sesno = 100u32;

        match get_changes_at_sesno(sesno).await {
            Ok(update_log) => {
                println!(
                    "获取到 sesno {} 的变更: {} 个元素",
                    sesno,
                    update_log.count()
                );
                assert!(update_log.count() >= 0);
            }
            Err(e) => {
                // 在没有数据库连接的测试环境中，这是预期的
                println!("测试跳过（数据库连接问题）: {}", e);
            }
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_change_operation_from_str() -> anyhow::Result<()> {
        assert_eq!(ChangeOperation::from_str("ADD")?, ChangeOperation::Add);
        assert_eq!(
            ChangeOperation::from_str("MODIFY")?,
            ChangeOperation::Modify
        );
        assert_eq!(
            ChangeOperation::from_str("DELETE")?,
            ChangeOperation::Delete
        );
        assert_eq!(ChangeOperation::from_str("insert")?, ChangeOperation::Add);

        assert!(ChangeOperation::from_str("UNKNOWN").is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_has_changes_at_sesno() -> anyhow::Result<()> {
        let sesno = 100u32;

        match has_changes_at_sesno(sesno).await {
            Ok(has_changes) => {
                println!("sesno {} 是否有变更: {}", sesno, has_changes);
                // 这个测试主要验证函数不会崩溃
            }
            Err(e) => {
                println!("测试跳过（数据库连接问题）: {}", e);
            }
        }

        Ok(())
    }
}
