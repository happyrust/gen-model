use super::finish_generation;

#[tokio::test]
async fn producer_failure_is_not_lost_when_receiver_reports_an_incomplete_run() {
    let generating = async { anyhow::bail!("cannot read frozen source") };
    let accepting = async { anyhow::bail!("generator omitted planned roots") };
    let error = finish_generation(generating, accepting).await.unwrap_err();
    let detail = format!("{error:#}");
    assert!(
        detail.starts_with("initialization generation failed: cannot read frozen source"),
        "{detail}"
    );
    assert!(detail.contains("cannot read frozen source"), "{detail}");
    assert!(
        detail.contains("generator omitted planned roots"),
        "{detail}"
    );
}

#[tokio::test]
async fn rejected_frozen_envelope_reports_its_cause_not_sender_shutdown() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let generating = async move {
        for item in 0..3 {
            tx.send(item)
                .await
                .map_err(|_| anyhow::anyhow!("initialization journal receiver closed"))?;
        }
        Ok(())
    };
    let accepting = async move {
        assert_eq!(rx.recv().await, Some(0));
        anyhow::bail!("frozen geometry outside root publication scope")
    };
    let error = finish_generation(generating, accepting).await.unwrap_err();
    assert!(format!("{error:#}").starts_with("initialization handoff failed: frozen geometry"));
    assert!(
        format!("{error:#}").contains("frozen geometry outside root publication scope"),
        "root cause was lost: {error:#}"
    );
}
