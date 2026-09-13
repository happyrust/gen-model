//! Shares one producer and its terminal result; cancelling a waiter leaves it running.
use std::{collections::HashMap, future::Future, sync::{Arc, Mutex, Weak}};
use tokio::sync::watch;

type Outcome<T> = Arc<anyhow::Result<T>>;
struct Flight<T> { result: watch::Sender<Option<Outcome<T>>> }
pub(crate) struct Flights<T> { entries: Mutex<HashMap<String, Weak<Flight<T>>>> }
impl<T> Default for Flights<T> {
    fn default() -> Self { Self { entries: Mutex::new(HashMap::new()) } }
}

impl<T: Clone + Send + Sync + 'static> Flights<T> {
    pub(crate) async fn run<F>(&self, key: String, work: F) -> anyhow::Result<T>
    where F: Future<Output = anyhow::Result<T>> + Send + 'static {
        let (flight, create) = {
            let mut entries = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            entries.retain(|_, value| value.strong_count() > 0);
            if let Some(flight) = entries.get(&key).and_then(Weak::upgrade) { (flight, false) }
            else {
                let flight = Arc::new(Flight { result: watch::channel(None).0 });
                entries.insert(key, Arc::downgrade(&flight));
                (flight, true)
            }
        };
        if create {
            let producer = flight.clone();
            tokio::spawn(async move {
                // A supervisor turns producer panic into a shared terminal error.
                let outcome = match tokio::spawn(work).await {
                    Ok(result) => result,
                    Err(error) => Err(anyhow::anyhow!("generation producer failed: {error}")),
                };
                producer.result.send_replace(Some(Arc::new(outcome)));
            });
        }
        let mut receiver = flight.result.subscribe();
        let outcome = loop {
            if let Some(result) = receiver.borrow_and_update().clone() { break result; }
            receiver.changed().await?;
        };
        match outcome.as_ref() {
            Ok(value) => Ok(value.clone()),
            Err(_) => {
                // Retain the shared outcome so the original typed error stays in the chain.
                Err(anyhow::Error::new(OutcomeError(outcome)))
            }
        }
    }
}

struct OutcomeError<T>(Outcome<T>);
impl<T> std::fmt::Debug for OutcomeError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("shared generation error") }
}
impl<T> std::fmt::Display for OutcomeError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.as_ref() { Err(error) => std::fmt::Display::fmt(error, f), Ok(_) => f.write_str("unexpected success") }
    }
}
impl<T> std::error::Error for OutcomeError<T> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.as_ref().as_ref().err().map(|error| error.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn concurrent_waiters_share_success_and_failure() {
        for fail in [false, true] {
            let flights = Arc::new(Flights::<usize>::default());
            let count = Arc::new(AtomicUsize::new(0));
            let gate = Arc::new(tokio::sync::Notify::new());
            let mut waiters = Vec::new();
            for _ in 0..8 {
                let (flights, count, gate) = (flights.clone(), count.clone(), gate.clone());
                waiters.push(tokio::spawn(async move { flights.run("same".into(), async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    gate.notified().await;
                    if fail { anyhow::bail!("fixture failure"); }
                    Ok(42)
                }).await }));
            }
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            gate.notify_one();
            for waiter in waiters { assert_eq!(waiter.await.unwrap().is_err(), fail); }
            assert_eq!(count.load(Ordering::SeqCst), 1);
        }
    }
    #[tokio::test]
    async fn cancelling_first_waiter_does_not_cancel_producer() {
        let flights = Arc::new(Flights::<usize>::default());
        let gate = Arc::new(tokio::sync::Notify::new());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (f, g) = (flights.clone(), gate.clone());
        let first = tokio::spawn(async move { f.run("same".into(), async move {
            started_tx.send(()).unwrap(); g.notified().await; Ok(7)
        }).await });
        started_rx.await.unwrap(); first.abort();
        gate.notify_one();
        assert_eq!(flights.run("same".into(), async { panic!("duplicate producer") }).await.unwrap(), 7);
    }
    #[tokio::test]
    async fn distinct_versions_do_not_share_a_result() {
        let flights = Flights::<usize>::default();
        let (first, second) = tokio::join!(
            flights.run("root:session1".into(), async { Ok(1) }),
            flights.run("root:session2".into(), async { Ok(2) }),
        );
        assert_eq!((first.unwrap(), second.unwrap()), (1, 2));
    }
    #[tokio::test]
    async fn panic_is_a_terminal_error_not_an_abandoned_waiter() {
        let flights = Flights::<usize>::default();
        let error = flights.run("panic".into(), async { panic!("fixture panic") }).await.unwrap_err();
        assert!(error.to_string().contains("generation producer failed"));
    }
    #[tokio::test]
    async fn shared_error_preserves_the_domain_cause() {
        let flights = Flights::<usize>::default();
        let error = flights.run("error".into(), async {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "fixture missing").into())
        }).await.unwrap_err();
        assert!(error.chain().any(|cause| cause.downcast_ref::<std::io::Error>()
            .is_some_and(|cause| cause.kind() == std::io::ErrorKind::NotFound)));
    }
}
