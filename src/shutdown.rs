use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
static STOP: AtomicBool = AtomicBool::new(false);
pub fn install() -> anyhow::Result<()> {
    ctrlc::set_handler(|| STOP.store(true, Ordering::Relaxed))?;
    Ok(())
}
pub fn stopped() -> bool {
    STOP.load(Ordering::Relaxed)
}
pub fn check() -> anyhow::Result<()> {
    anyhow::ensure!(!stopped(), "操作已取消");
    Ok(())
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
