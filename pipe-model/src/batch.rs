//! 变更批（spec 决策 2/3）：服务唯一产出物。
//!
//! `MutationBatch` 内的操作与 `e3d_collector` 落库行同构——渲染成同样的
//! `pe`/`ATT_*` 行交给 persist_writer 后，下游无法区分编辑行与会话行。
//! 批内顺序即提交顺序；整个批 = 一个命名撤销边界（UNDOABLE 等价物）。

use aios_core::{NamedAttrMap, RefU64};
use serde::{Deserialize, Serialize};

/// 单个行级变更操作。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MutationOp {
    /// `NEW <type> <name>`：`refno` 为编辑段分配的标识（spec 决策：source=edit
    /// 隔离段——同批后续 op 如 `under`/`Include` 引用它），`noun` 为元素类型名，
    /// `under` 为属主 refno，`name` 可空（空=未命名）。
    Create {
        refno: RefU64,
        noun: String,
        under: RefU64,
        name: Option<String>,
        /// 建后立即生效的属性（等价 PML 创建序列里紧跟着的属性直赋）。
        attrs: NamedAttrMap,
    },
    /// `DELETE`/`KILL` 语义。
    Delete { element: RefU64 },
    /// 改名；`cascade=true` 时级联重命名所有名字前缀匹配的子元素
    /// （pipecreation changeName 语义）。
    Rename { element: RefU64, name: String, cascade: bool },
    /// 属性行写入（等价 ATTRIB 直赋 / `$!<attr> <value>`）。
    SetAttrs { element: RefU64, attrs: NamedAttrMap },
    /// 成员关系调整：`INCLUDE` 进 `into` 的成员表；`order` 为目标序位
    /// （分段/合并的成员迁移走这里，`None` = 追加）。
    Include { element: RefU64, into: RefU64, order: Option<usize> },
    /// 从属主成员表移除。
    Remove { element: RefU64 },
}

/// 提交前需要 UI 确认的项（四级错误语义的入口，见 [`crate::PipeError`]）。
#[derive(Debug, Clone)]
pub struct Confirm {
    /// 消息码风格提示（E3D confirm/question 文案语义）。
    pub prompt: String,
    /// 确认后附带执行的备选 op（如 FCONN 强接）。
    pub on_yes: Vec<MutationOp>,
}

/// 一次操作的完整产出：有序 op 序列 + 待确认项 + 结构化警告。
#[derive(Debug, Default)]
pub struct MutationBatch {
    /// 撤销组名（等价 `undo.description(...)`）。
    pub name: String,
    pub ops: Vec<MutationOp>,
    /// 调用方提交前必须逐条与 UI/用户确认的项；不空时调用方不应直接落库。
    pub requires_confirm: Vec<Confirm>,
    /// 批级警告：E3D 静默容错点位在此显式化（spec 决策 10）——
    /// 规格 dbRef 化失败跳过、连接后回读失败、级联收集为空等，不吞。
    pub warnings: Vec<crate::outcome::PipeWarning>,
}

impl MutationBatch {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ops: Vec::new(),
            requires_confirm: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// `undo.add`/`endUndoable` 组合器（spec 决策 3）：把若干已产出的批压成
    /// 一个命名撤销边界——跨操作回滚（如 excess 警告整批滚）走显式 API，
    /// 不靠客户端手工拼。`ops`/`requires_confirm`/`warnings` 均按批序原样
    /// 拼接，提交顺序不变；组名 = UNDOABLE description，子批名不保留
    /// （撤销粒度到组）。
    pub fn undo_group(name: impl Into<String>, batches: Vec<MutationBatch>) -> Self {
        let mut group = Self::named(name);
        for batch in batches {
            group.ops.extend(batch.ops);
            group.requires_confirm.extend(batch.requires_confirm);
            group.warnings.extend(batch.warnings);
        }
        group
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::PipeWarning;

    fn op(refno: u64) -> MutationOp {
        MutationOp::Delete {
            element: RefU64(refno),
        }
    }

    /// 组批把子批 op/confirm/warning 按序压平成单一边界（spec 决策 3）。
    #[test]
    fn undo_group_flattens_in_batch_order() {
        let mut a = MutationBatch::named("create_pipe");
        a.ops.push(op(1));
        a.ops.push(op(2));
        a.requires_confirm.push(Confirm {
            prompt: "替换既有断点".into(),
            on_yes: vec![op(3)],
        });
        let mut b = MutationBatch::named("connect_end");
        b.ops.push(op(4));
        b.warnings.push(PipeWarning {
            message: "连接后回读失败".into(),
            side_effect: None,
        });

        let group = MutationBatch::undo_group("编辑段 1", vec![a, b]);
        assert_eq!(group.name, "编辑段 1");
        assert_eq!(group.ops.len(), 3);
        match (&group.ops[0], &group.ops[2]) {
            (MutationOp::Delete { element: e1 }, MutationOp::Delete { element: e2 }) => {
                assert_eq!((e1.0, e2.0), (1, 4), "op 顺序 = 批序 + 批内序");
            }
            _ => panic!("ops must flatten in order"),
        }
        assert_eq!(group.requires_confirm.len(), 1);
        assert_eq!(group.warnings.len(), 1);
    }

    /// 空组 = 只有名字的边界（合法——E3D 空 UNDOABLE 也成立）。
    #[test]
    fn undo_group_empty_is_named_boundary() {
        let group = MutationBatch::undo_group("noop", vec![]);
        assert_eq!(group.name, "noop");
        assert!(group.ops.is_empty() && group.requires_confirm.is_empty() && group.warnings.is_empty());
    }
}
