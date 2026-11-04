use anyhow::{Result, ensure};
use serde::Deserialize;
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct NoAuthPeriod {
    pub enabled: bool,
    pub weekdays: Vec<u32>,
    pub start: String,
    pub end: String,
}
impl Default for NoAuthPeriod {
    fn default() -> Self {
        Self {
            enabled: false,
            weekdays: vec![0, 1, 2, 3, 4],
            start: "23:30".into(),
            end: "05:30".into(),
        }
    }
}
fn minute(s: &str) -> Result<i64> {
    let b = s.as_bytes();
    ensure!(
        b.len() == 5 && b[2] == b':' && [b[0], b[1], b[3], b[4]].iter().all(u8::is_ascii_digit),
        "时间必须为 HH:MM"
    );
    let h = ((b[0] - b'0') * 10 + b[1] - b'0') as i64;
    let m = ((b[3] - b'0') * 10 + b[4] - b'0') as i64;
    ensure!(h < 24 && m < 60, "时间超出范围");
    Ok(h * 60 + m)
}
impl NoAuthPeriod {
    pub fn validate(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        ensure!(
            !self.weekdays.is_empty() && self.weekdays.iter().all(|d| *d <= 6),
            "weekdays 必须为 0～6，且不能为空"
        );
        ensure!(
            minute(&self.start)? != minute(&self.end)?,
            "开始和结束时间不能相同"
        );
        Ok(())
    }
    /// UTC epoch seconds; schedule is always Beijing time, independent of host timezone.
    pub fn next_window(&self, now: i64) -> Option<(i64, i64)> {
        if !self.enabled {
            return None;
        }
        let start = minute(&self.start).ok()? * 60;
        let end = minute(&self.end).ok()? * 60;
        let day = (now + 8 * 3600).div_euclid(86400);
        for offset in -1..=7 {
            let date = day + offset;
            if !self.weekdays.contains(&((date + 4).rem_euclid(7) as u32)) {
                continue;
            }
            let midnight = date * 86400 - 8 * 3600;
            let a = midnight + start;
            let b = midnight + end + if end < start { 86400 } else { 0 };
            if now < b {
                return Some((a, b));
            }
        }
        None
    }
}
