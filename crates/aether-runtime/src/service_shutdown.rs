use super::*;

impl RunService {
    /// Stops acceptance and waits asynchronously, at most grace, for real finalization.
    pub async fn shutdown(&self, grace: Duration) {
        self.closing.store(true, Ordering::Release);
        self.maintenance_cancel.cancel();
        {
            for (id, record) in &mut lock(&self.registry).records {
                if let Some(cancel) = &record.cancel {
                    record.snapshot.cancel_requested = true;
                    cancel.cancel();
                    self.approvals.remove_run(id);
                }
            }
        }
        let deadline = tokio::time::Instant::now() + grace;
        let drained = tokio::time::timeout_at(deadline, async {
            let _drain = self.drain.lock().await;
            while self.preparing.load(Ordering::Acquire) != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            while std::future::poll_fn(|cx| {
                let mut tasks = lock(&self.tasks);
                let Some(task) = tasks.last_mut() else {
                    return Poll::Ready(false);
                };
                match std::pin::Pin::new(task).poll(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(_) => {
                        tasks.pop();
                        Poll::Ready(true)
                    }
                }
            })
            .await
            {}
            std::future::poll_fn(|cx| {
                let mut maintenance = lock(&self.maintenance);
                let Some(task) = maintenance.as_mut() else {
                    return Poll::Ready(());
                };
                match std::pin::Pin::new(task).poll(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(_) => {
                        maintenance.take();
                        Poll::Ready(())
                    }
                }
            })
            .await;
        })
        .await;
        let active = lock(&self.registry)
            .records
            .values()
            .filter(|record| record.cancel.is_some())
            .count();
        let preparing = self.preparing.load(Ordering::Acquire);
        let left = lock(&self.tasks)
            .iter()
            .filter(|task| !task.is_finished())
            .count()
            + usize::from(
                lock(&self.maintenance)
                    .as_ref()
                    .is_some_and(|task| !task.is_finished()),
            );
        if drained.is_err() {
            tracing::warn!(active, preparing, left, "runtime.shutdown.incomplete");
        } else {
            tracing::info!(active, preparing, left, "runtime.shutdown.complete");
        }
    }
}
