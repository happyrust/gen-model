//! Process-wide geometry concurrency gate.
//!
//! Drivers split work without holding a permit. Each geometry work unit then
//! enters through [`run_geometry`]. A task must never acquire the gate twice.

use std::sync::OnceLock;

use tokio::sync::Semaphore;

static GATE: OnceLock<Semaphore> = OnceLock::new();

pub fn permits() -> usize {
    crate::options::geometry_permits()
}

fn gate() -> &'static Semaphore {
    GATE.get_or_init(|| Semaphore::new(permits()))
}

pub fn is_geometry_task() -> bool {
    NESTED.try_with(|_| ()).is_ok()
}

pub fn fan_out_width(work_items: usize) -> usize {
    fan_out_width_at(permits(), work_items)
}

fn fan_out_width_at(quota: usize, work_items: usize) -> usize {
    quota.min(work_items.max(1)).max(1)
}

pub async fn run_geometry<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    run_geometry_on(gate(), future).await
}

/// Enter the process geometry budget unless the current task already owns a
/// permit. Shared single-flight loaders use this to avoid a second semaphore
/// and to remain safe when an on-demand miss originates inside geometry work.
pub async fn run_geometry_shared<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    if NESTED.try_with(|_| ()).is_ok() {
        future.await
    } else {
        run_geometry(future).await
    }
}

async fn run_geometry_on<F, T>(gate: &Semaphore, future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    assert!(
        NESTED.try_with(|_| ()).is_err(),
        "geometry concurrency permit is not reentrant"
    );
    let _permit = gate
        .acquire()
        .await
        .expect("geometry concurrency gate is never closed");
    NESTED.scope((), future).await
}

tokio::task_local! {
    static NESTED: ();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn one_permit_is_the_serial_rollback_mode() {
        for count in [0, 1, 2, 16, 100] {
            assert_eq!(fan_out_width_at(1, count), 1);
        }
    }

    #[tokio::test]
    async fn active_geometry_never_exceeds_the_gate() {
        let gate = Arc::new(Semaphore::new(2));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let futures = (0..8).map(|_| {
            let gate = gate.clone();
            let active = active.clone();
            let peak = peak.clone();
            async move {
                run_geometry_on(&gate, async {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    active.fetch_sub(1, Ordering::SeqCst);
                })
                .await;
            }
        });
        futures::future::join_all(futures).await;
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }
}
