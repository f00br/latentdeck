use std::future::Future;

pub(crate) async fn preflight_before_shutdown<T, E, P, S>(preflight: P, shutdown: S) -> Result<T, E>
where
    P: Future<Output = Result<T, E>>,
    S: Future<Output = Result<(), E>>,
{
    let prepared = preflight.await?;
    shutdown.await?;
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::preflight_before_shutdown;

    #[tokio::test]
    async fn failed_preflight_does_not_shutdown_the_current_runtime() {
        let shutdown_called = Arc::new(AtomicBool::new(false));
        let shutdown_observer = Arc::clone(&shutdown_called);

        let result =
            preflight_before_shutdown(async { Err::<(), _>("invalid source") }, async move {
                shutdown_observer.store(true, Ordering::SeqCst);
                Ok(())
            })
            .await;

        assert_eq!(result, Err("invalid source"));
        assert!(!shutdown_called.load(Ordering::SeqCst));
    }
}
