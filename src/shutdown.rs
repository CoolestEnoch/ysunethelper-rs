use std::{
    future::Future,
    sync::atomic::{AtomicBool, Ordering},
    sync::mpsc::{Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};
pub const POLL_INTERVAL: Duration = Duration::from_millis(50);
static STOP: AtomicBool = AtomicBool::new(false);
#[derive(Debug, thiserror::Error)]
#[error("操作已取消")]
pub struct Cancelled;
pub fn install() -> anyhow::Result<()> {
    ctrlc::set_handler(|| STOP.store(true, Ordering::Relaxed))?;
    Ok(())
}
pub fn stopped() -> bool {
    STOP.load(Ordering::Relaxed)
}
pub fn check() -> anyhow::Result<()> {
    if stopped() {
        return Err(Cancelled.into());
    }
    Ok(())
}
pub fn receive<T>(receiver: &Receiver<T>) -> anyhow::Result<T> {
    loop {
        check()?;
        match receiver.recv_timeout(POLL_INTERVAL) {
            Ok(value) => {
                check()?;
                return Ok(value);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => anyhow::bail!("输入线程已结束"),
        }
    }
}

/// Drive cancellable network futures while keeping the protocol API synchronous.
pub struct Runtime(Option<tokio::runtime::Runtime>);
impl Runtime {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self(Some(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
        )))
    }
    pub fn block_on<F: Future>(&self, future: F) -> anyhow::Result<F::Output> {
        self.0.as_ref().expect("runtime is alive").block_on(async {
            let mut future = std::pin::pin!(future);
            loop {
                check()?;
                if let Ok(value) = tokio::time::timeout(POLL_INTERVAL, &mut future).await {
                    check()?;
                    return Ok(value);
                }
            }
        })
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        // A system DNS lookup may still be running in Tokio's blocking pool.
        // Do not let runtime destruction wait for it after cancelling the request.
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}
pub fn wait(duration: Duration) -> bool {
    let until = Instant::now() + duration;
    while !stopped() {
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(100)));
    }
    false
}
