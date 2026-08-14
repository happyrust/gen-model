//! 模型实例保存的有界合批、不可变计划与统一 receiver。
//!
//! 生产者的小尾批必须保持原样进入 [`FrozenShapeBatch`]；这里刻意不调用
//! `ShapeInstancesData::merge/merge_ref`，因为那两个接口不保留全部关系字段。

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use aios_core::RefnoEnum;
use aios_core::geometry::ShapeInstancesData;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SaveConflict {
    RecordContent {
        kind: &'static str,
        record_id: String,
    },
    NormalTubiOverlap {
        record_id: String,
    },
    NonFiniteTransform {
        kind: &'static str,
        record_id: String,
    },
    MissingTubiAabb {
        record_id: String,
    },
}

impl std::fmt::Display for SaveConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RecordContent { kind, record_id } => {
                write!(formatter, "持久化记录 {record_id} 出现不同内容（{kind}）")
            }
            Self::NormalTubiOverlap { record_id } => write!(
                formatter,
                "同一个 inst_relate id 同时来自普通元素与隐含直管段: {record_id}"
            ),
            Self::NonFiniteTransform { kind, record_id } => {
                write!(formatter, "{kind} {record_id} 含 NaN transform")
            }
            Self::MissingTubiAabb { record_id } => {
                write!(formatter, "隐含直管段 {record_id} 缺少 AABB")
            }
        }
    }
}

impl std::error::Error for SaveConflict {}

pub(crate) const SOFT_INSTANCE_ROWS: usize = 300;
pub(crate) const SOFT_GEO_OCCURRENCES: usize = 1_200;
pub(crate) const HARD_INSTANCE_ROWS: usize = 1_000;
pub(crate) const HARD_GEO_OCCURRENCES: usize = 4_000;
pub(crate) const HARD_SOURCE_BATCHES: usize = 32;
pub(crate) const HARD_ESTIMATED_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const IDLE_FLUSH: Duration = Duration::from_millis(2);
pub(crate) const MAX_FLUSH_WAIT: Duration = Duration::from_millis(8);
pub(crate) const SQL_PACKET_ROWS: usize = 300;
pub(crate) const SQL_PACKET_BYTES: usize = 1024 * 1024;
pub(crate) const DIRECT_MAX_IN_FLIGHT: usize = 4;

static PRODUCER_BLOCKED_NANOS: AtomicU64 = AtomicU64::new(0);

/// 统一记录生产者在有界 channel 上的背压等待。
pub(crate) async fn send_shape_batch(
    sender: &flume::Sender<ShapeInstancesData>,
    batch: ShapeInstancesData,
) -> Result<(), flume::SendError<ShapeInstancesData>> {
    let started = Instant::now();
    let result = sender.send_async(batch).await;
    let nanos = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
    PRODUCER_BLOCKED_NANOS.fetch_add(nanos, Ordering::Relaxed);
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveMode {
    TargetedReplace,
    FullBuild,
}

impl SaveMode {
    pub(crate) fn replaces_existing(self) -> bool {
        matches!(self, Self::TargetedReplace)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlushReason {
    SoftLimit,
    HardLimit,
    NextBatchWouldOverflow,
    Idle,
    MaxWait,
    ChannelClosed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BatchMeasure {
    pub instance_rows: usize,
    pub geo_occurrences: usize,
    pub estimated_bytes: usize,
}

impl BatchMeasure {
    fn for_batch(batch: &ShapeInstancesData) -> anyhow::Result<Self> {
        let geo_occurrences = batch
            .inst_geos_map
            .values()
            .map(|data| data.insts.len())
            .sum();
        let relation_count = batch.neg_relate_map.values().map(Vec::len).sum::<usize>()
            + batch
                .ngmr_neg_relate_map
                .values()
                .map(Vec::len)
                .sum::<usize>();
        // serde 覆盖所有可持久字段；neg_relate 被上游标成 serde(skip)，按每个引用
        // 额外预留 32 字节，确保上限只会保守提前 flush。
        let estimated_bytes = serde_json::to_vec(batch)
            .map_err(|error| anyhow::anyhow!("估算 ShapeInstancesData 载荷失败: {error}"))?
            .len()
            .saturating_add(relation_count.saturating_mul(32));
        Ok(Self {
            instance_rows: batch.inst_cnt(),
            geo_occurrences,
            estimated_bytes,
        })
    }

    fn checked_add(self, other: Self) -> anyhow::Result<Self> {
        Ok(Self {
            instance_rows: self
                .instance_rows
                .checked_add(other.instance_rows)
                .ok_or_else(|| anyhow::anyhow!("实例行计数溢出"))?,
            geo_occurrences: self
                .geo_occurrences
                .checked_add(other.geo_occurrences)
                .ok_or_else(|| anyhow::anyhow!("几何 occurrence 计数溢出"))?,
            estimated_bytes: self
                .estimated_bytes
                .checked_add(other.estimated_bytes)
                .ok_or_else(|| anyhow::anyhow!("载荷估算计数溢出"))?,
        })
    }
}

/// 一次 flush 的冻结输入。保存原始批，不做有损 merge。
#[derive(Debug, Default)]
pub(crate) struct FrozenShapeBatch {
    batches: Vec<ShapeInstancesData>,
    measure: BatchMeasure,
}

impl FrozenShapeBatch {
    #[cfg(test)]
    pub(crate) fn from_batches_for_test(batches: Vec<ShapeInstancesData>) -> anyhow::Result<Self> {
        let mut frozen = Self::default();
        for batch in batches {
            let measure = BatchMeasure::for_batch(&batch)?;
            frozen.push_measured(batch, measure)?;
        }
        Ok(frozen)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }

    pub(crate) fn source_batch_count(&self) -> usize {
        self.batches.len()
    }

    pub(crate) fn instance_rows(&self) -> usize {
        self.measure.instance_rows
    }

    pub(crate) fn geo_occurrences(&self) -> usize {
        self.measure.geo_occurrences
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        self.measure.estimated_bytes
    }

    pub(crate) fn batches(&self) -> &[ShapeInstancesData] {
        &self.batches
    }

    fn would_exceed_hard(&self, next: BatchMeasure) -> bool {
        if self.is_empty() {
            return false;
        }
        let Ok(total) = self.measure.checked_add(next) else {
            return true;
        };
        total.instance_rows > HARD_INSTANCE_ROWS
            || total.geo_occurrences > HARD_GEO_OCCURRENCES
            || self.batches.len() + 1 > HARD_SOURCE_BATCHES
            || total.estimated_bytes > HARD_ESTIMATED_BYTES
    }

    fn push_measured(
        &mut self,
        batch: ShapeInstancesData,
        measure: BatchMeasure,
    ) -> anyhow::Result<()> {
        self.measure = self.measure.checked_add(measure)?;
        self.batches.push(batch);
        Ok(())
    }

    fn reached_soft(&self) -> bool {
        self.measure.instance_rows >= SOFT_INSTANCE_ROWS
            || self.measure.geo_occurrences >= SOFT_GEO_OCCURRENCES
    }

    fn reached_hard(&self) -> bool {
        self.measure.instance_rows >= HARD_INSTANCE_ROWS
            || self.measure.geo_occurrences >= HARD_GEO_OCCURRENCES
            || self.batches.len() >= HARD_SOURCE_BATCHES
            || self.measure.estimated_bytes >= HARD_ESTIMATED_BYTES
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SavePhase {
    SharedContent,
    Relations,
    InstanceRelations,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SqlPacket {
    pub phase: SavePhase,
    pub sql: String,
    pub row_count: usize,
    pub estimated_bytes: usize,
}

#[derive(Debug)]
pub(crate) struct SavePlan {
    pub(crate) mode: SaveMode,
    pub(crate) flush_reason: FlushReason,
    pub(crate) source_batch_count: usize,
    pub(crate) instance_rows: usize,
    pub(crate) geo_occurrences: usize,
    pub(crate) coalesce_wait: Duration,
    pub(crate) delete_refnos: Vec<RefnoEnum>,
    pub(crate) written_refnos: Vec<RefnoEnum>,
    pub(crate) packets: Vec<SqlPacket>,
    pub(crate) metadata_query_count: usize,
    pub(crate) conflict_count: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct SaveOutcome {
    pub written_refnos: Vec<RefnoEnum>,
    pub source_batch_count: usize,
    pub flush_reason: FlushReason,
    pub instance_rows: usize,
    pub geo_occurrences: usize,
    pub coalesce_wait: Duration,
    pub metadata_query_count: usize,
    pub sql_packet_count: usize,
    pub sql_bytes: usize,
    pub scoped_delete_count: usize,
    pub conflict_count: usize,
}

#[derive(Debug, Default)]
pub(crate) struct ShapeSaveRunOutcome {
    pub written_refnos: HashSet<RefnoEnum>,
    pub source_batch_count: usize,
    pub flush_count: usize,
    pub instance_rows: usize,
    pub geo_occurrences: usize,
    pub coalesce_wait: Duration,
    pub metadata_query_count: usize,
    pub sql_packet_count: usize,
    pub sql_bytes: usize,
    pub scoped_delete_count: usize,
    pub conflict_count: usize,
    pub producer_blocked: Duration,
}

impl ShapeSaveRunOutcome {
    fn record(&mut self, outcome: SaveOutcome) {
        self.written_refnos.extend(outcome.written_refnos);
        self.source_batch_count += outcome.source_batch_count;
        self.flush_count += 1;
        self.instance_rows += outcome.instance_rows;
        self.geo_occurrences += outcome.geo_occurrences;
        self.coalesce_wait += outcome.coalesce_wait;
        self.metadata_query_count += outcome.metadata_query_count;
        self.sql_packet_count += outcome.sql_packet_count;
        self.sql_bytes += outcome.sql_bytes;
        self.scoped_delete_count += outcome.scoped_delete_count;
        self.conflict_count += outcome.conflict_count;
    }
}

async fn flush_batch(
    frozen: FrozenShapeBatch,
    mode: SaveMode,
    reason: FlushReason,
    coalesce_wait: Duration,
) -> anyhow::Result<SaveOutcome> {
    let plan = super::pdms_inst::build_save_plan(&frozen, mode, reason, coalesce_wait).await?;
    super::pdms_inst::execute_save_plan(plan).await
}

/// 定向与全量生成共用的唯一实例保存 consumer。
pub(crate) async fn run_shape_save_receiver(
    receiver: flume::Receiver<ShapeInstancesData>,
    mode: SaveMode,
) -> anyhow::Result<ShapeSaveRunOutcome> {
    let producer_blocked_at_start = PRODUCER_BLOCKED_NANOS.load(Ordering::Relaxed);
    let mut run = ShapeSaveRunOutcome::default();
    let mut pending = None;

    loop {
        let first = match pending.take() {
            Some(batch) => batch,
            None => match receiver.recv_async().await {
                Ok(batch) => batch,
                Err(_) => break,
            },
        };

        let first_at = Instant::now();
        let mut last_receive_at = first_at;
        let first_measure = BatchMeasure::for_batch(&first)?;
        let mut frozen = FrozenShapeBatch::default();
        frozen.push_measured(first, first_measure)?;
        let reason = loop {
            if frozen.reached_hard() {
                break FlushReason::HardLimit;
            }
            if frozen.reached_soft() {
                break FlushReason::SoftLimit;
            }

            let idle_left = IDLE_FLUSH.saturating_sub(last_receive_at.elapsed());
            let absolute_left = MAX_FLUSH_WAIT.saturating_sub(first_at.elapsed());
            let wait = idle_left.min(absolute_left);
            if wait.is_zero() {
                break if absolute_left.is_zero() {
                    FlushReason::MaxWait
                } else {
                    FlushReason::Idle
                };
            }

            match tokio::time::timeout(wait, receiver.recv_async()).await {
                Ok(Ok(next)) => {
                    let next_measure = BatchMeasure::for_batch(&next)?;
                    if frozen.would_exceed_hard(next_measure) {
                        pending = Some(next);
                        break FlushReason::NextBatchWouldOverflow;
                    }
                    frozen.push_measured(next, next_measure)?;
                    last_receive_at = Instant::now();
                }
                Ok(Err(_)) => break FlushReason::ChannelClosed,
                Err(_) => {
                    break if first_at.elapsed() >= MAX_FLUSH_WAIT {
                        FlushReason::MaxWait
                    } else {
                        FlushReason::Idle
                    };
                }
            }
        };

        let outcome = flush_batch(frozen, mode, reason, first_at.elapsed()).await?;
        let channel_closed = reason == FlushReason::ChannelClosed;
        println!(
            "shape_save_flush mode={mode:?} reason={:?} source_batches={} instances={} geos={} wait_ms={} metadata_queries={} sql_packets={} sql_bytes={} scoped_deletes={} conflicts={}",
            outcome.flush_reason,
            outcome.source_batch_count,
            outcome.instance_rows,
            outcome.geo_occurrences,
            outcome.coalesce_wait.as_millis(),
            outcome.metadata_query_count,
            outcome.sql_packet_count,
            outcome.sql_bytes,
            outcome.scoped_delete_count,
            outcome.conflict_count,
        );
        run.record(outcome);
        if channel_closed && pending.is_none() {
            break;
        }
    }

    run.producer_blocked = Duration::from_nanos(
        PRODUCER_BLOCKED_NANOS
            .load(Ordering::Relaxed)
            .saturating_sub(producer_blocked_at_start),
    );
    println!(
        "shape_save_summary mode={mode:?} source_batches={} flushes={} instances={} geos={} coalesce_wait_ms={} metadata_queries={} sql_packets={} sql_bytes={} scoped_deletes={} conflicts={} producer_blocked_ms={}",
        run.source_batch_count,
        run.flush_count,
        run.instance_rows,
        run.geo_occurrences,
        run.coalesce_wait.as_millis(),
        run.metadata_query_count,
        run.sql_packet_count,
        run.sql_bytes,
        run.scoped_delete_count,
        run.conflict_count,
        run.producer_blocked.as_millis(),
    );

    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(instances: usize, geos: usize, bytes: usize) -> BatchMeasure {
        BatchMeasure {
            instance_rows: instances,
            geo_occurrences: geos,
            estimated_bytes: bytes,
        }
    }

    #[test]
    fn next_batch_is_rejected_before_crossing_each_hard_limit() {
        let mut frozen = FrozenShapeBatch::default();
        frozen.measure = measure(HARD_INSTANCE_ROWS - 1, 1, 1);
        frozen.batches.push(ShapeInstancesData::default());
        assert!(frozen.would_exceed_hard(measure(2, 0, 0)));

        frozen.measure = measure(1, HARD_GEO_OCCURRENCES - 1, 1);
        assert!(frozen.would_exceed_hard(measure(0, 2, 0)));

        frozen.measure = measure(1, 1, HARD_ESTIMATED_BYTES - 1);
        assert!(frozen.would_exceed_hard(measure(0, 0, 2)));

        frozen.measure = measure(1, 1, 1);
        frozen.batches = (0..HARD_SOURCE_BATCHES)
            .map(|_| ShapeInstancesData::default())
            .collect();
        assert!(frozen.would_exceed_hard(measure(0, 0, 0)));
    }

    #[test]
    fn one_oversized_source_batch_is_accepted_then_immediately_flushed() {
        let frozen = FrozenShapeBatch::default();
        assert!(!frozen.would_exceed_hard(measure(
            HARD_INSTANCE_ROWS + 1,
            HARD_GEO_OCCURRENCES + 1,
            HARD_ESTIMATED_BYTES + 1,
        )));
    }

    #[test]
    fn soft_limits_are_inclusive() {
        let mut frozen = FrozenShapeBatch::default();
        frozen.measure = measure(SOFT_INSTANCE_ROWS, 0, 0);
        assert!(frozen.reached_soft());
        frozen.measure = measure(0, SOFT_GEO_OCCURRENCES, 0);
        assert!(frozen.reached_soft());
    }

    #[tokio::test]
    async fn channel_close_flushes_all_source_batches_once() {
        let (sender, receiver) = flume::bounded(32);
        for _ in 0..16 {
            sender
                .send_async(ShapeInstancesData::default())
                .await
                .expect("fixture send");
        }
        drop(sender);

        let outcome = run_shape_save_receiver(receiver, SaveMode::FullBuild)
            .await
            .expect("receiver flush");
        assert_eq!(outcome.source_batch_count, 16);
        assert_eq!(outcome.flush_count, 1);
        assert_eq!(outcome.sql_packet_count, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn idle_timer_flushes_without_waiting_for_channel_close() {
        let (sender, receiver) = flume::bounded(4);
        sender
            .send_async(ShapeInstancesData::default())
            .await
            .expect("first send");
        let task = tokio::spawn(run_shape_save_receiver(receiver, SaveMode::FullBuild));
        // Windows 默认计时粒度约 15.6ms；留足两个 tick，避免测试与 2ms timer 同拍。
        tokio::time::sleep(Duration::from_millis(40)).await;
        sender
            .send_async(ShapeInstancesData::default())
            .await
            .expect("second send");
        drop(sender);

        let outcome = task.await.expect("join").expect("receiver");
        assert_eq!(outcome.source_batch_count, 2);
        assert_eq!(outcome.flush_count, 2);
    }

    #[tokio::test]
    async fn source_batch_hard_limit_flushes_without_losing_the_tail() {
        let (sender, receiver) = flume::bounded(HARD_SOURCE_BATCHES + 1);
        for _ in 0..=HARD_SOURCE_BATCHES {
            sender
                .send_async(ShapeInstancesData::default())
                .await
                .expect("fixture send");
        }
        drop(sender);

        let outcome = run_shape_save_receiver(receiver, SaveMode::FullBuild)
            .await
            .expect("receiver flush");
        assert_eq!(outcome.source_batch_count, HARD_SOURCE_BATCHES + 1);
        assert_eq!(outcome.flush_count, 2);
    }
}
