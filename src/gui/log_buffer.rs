//! 流れてくる行を、上限つきで持つ。

use std::collections::VecDeque;

/// 画面が持つ行数の上限。
pub const LOG_CAP: usize = 10_000;

#[derive(Debug, Clone)]
pub struct LogBuffer {
    cap: usize,
    lines: VecDeque<String>,
}

impl LogBuffer {
    pub fn new(cap: usize) -> Self {
        LogBuffer {
            cap,
            lines: VecDeque::new(),
        }
    }

    /// 上限を超えたら、古い行から捨てる。
    pub fn push(&mut self, line: String) {
        if self.lines.len() >= self.cap {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

impl Default for LogBuffer {
    fn default() -> Self {
        LogBuffer::new(LOG_CAP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_buffer_caps_at_10000() {
        let mut b = LogBuffer::new(LOG_CAP);
        for i in 1..=25_000 {
            b.push(format!("line {i}"));
        }
        assert_eq!(b.lines().count(), 10_000);
        assert_eq!(b.lines().next(), Some("line 15001"));
        assert_eq!(b.lines().last(), Some("line 25000"));
        assert_eq!(b.len(), 10_000);
    }

    #[test]
    fn keeps_everything_below_the_cap_and_can_be_cleared() {
        let mut b = LogBuffer::new(3);
        assert!(b.is_empty());
        b.push("a".into());
        b.push("b".into());
        b.push("c".into());
        assert_eq!(b.lines().collect::<Vec<_>>(), ["a", "b", "c"]);
        b.push("d".into());
        assert_eq!(b.lines().collect::<Vec<_>>(), ["b", "c", "d"]);
        b.clear();
        assert!(b.is_empty());
    }

    #[test]
    fn default_uses_the_screen_cap() {
        let mut b = LogBuffer::default();
        for i in 0..=LOG_CAP {
            b.push(i.to_string());
        }
        assert_eq!(b.len(), LOG_CAP);
    }
}
