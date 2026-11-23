//! 条件が成り立つまでの待ち。
use std::time::{Duration, Instant};

/// 条件が成り立つまで`step`ごとに試す。上限を過ぎても成り立たなければ偽。最初の1回は必ず試す。
pub fn wait_until(limit: Duration, step: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    loop {
        if ok() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_until_tries_until_true_or_the_limit() {
        let mut n = 0;
        assert!(wait_until(Duration::from_secs(5), Duration::ZERO, || {
            n += 1;
            n == 3
        }));
        assert_eq!(n, 3);
        let mut tries = 0;
        assert!(!wait_until(Duration::ZERO, Duration::ZERO, || {
            tries += 1;
            false
        }));
        assert_eq!(tries, 1);
        let start = Instant::now();
        assert!(!wait_until(
            Duration::from_millis(50),
            Duration::from_millis(10),
            || false
        ));
        assert!(start.elapsed() >= Duration::from_millis(50));
    }
}
