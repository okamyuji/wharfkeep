//! `docker logs` の出力を行に直す。
//!
//! Docker は出力をバイト列の塊で送る。塊の切れ目は行の切れ目と関係がないので、
//! 行の途中で切れた分は次の塊が来るまで持ち越す。

use super::DockerError;
use bollard::container::LogOutput;
use futures_util::{Stream, StreamExt};
use std::collections::VecDeque;

/// 1行として持つ最大のバイト数。
pub const LINE_CAP: usize = 64 * 1024;

/// 塊を受けて、完結した行だけを返す。
#[derive(Debug, Default)]
pub struct LineSplitter {
    buf: Vec<u8>,
}

impl LineSplitter {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        let mut rest = chunk;
        while let Some(i) = rest.iter().position(|&b| b == b'\n') {
            self.extend_capped(&rest[..i]);
            // 切れ目が `\r` と `\n` の間に来ることがあるので、`\r` は行が閉じてから外す。
            if self.buf.last() == Some(&b'\r') {
                self.buf.pop();
            }
            lines.push(String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned());
            rest = &rest[i + 1..];
        }
        self.extend_capped(rest);
        lines
    }

    /// 改行のない出力が続いてもメモリが増え続けないよう、`LINE_CAP`を超えた分は捨てる。
    fn extend_capped(&mut self, bytes: &[u8]) {
        let room = LINE_CAP.saturating_sub(self.buf.len());
        self.buf.extend_from_slice(&bytes[..bytes.len().min(room)]);
    }

    /// 改行で終わっていない最後の行。なければ `None`。
    pub fn finish(self) -> Option<String> {
        (!self.buf.is_empty()).then(|| String::from_utf8_lossy(&self.buf).into_owned())
    }
}

struct LogState<S> {
    inner: S,
    out: LineSplitter,
    err: LineSplitter,
    ready: VecDeque<Result<String, DockerError>>,
    done: bool,
}

/// stdout と stderr は別々に持ち越す。混ぜると、片方の行の途中にもう片方が入る。
pub fn split_log_stream<S>(inner: S) -> impl Stream<Item = Result<String, DockerError>>
where
    S: Stream<Item = Result<LogOutput, bollard::errors::Error>> + Unpin,
{
    let state = LogState {
        inner,
        out: LineSplitter::default(),
        err: LineSplitter::default(),
        ready: VecDeque::new(),
        done: false,
    };
    futures_util::stream::unfold(state, |mut st| async move {
        loop {
            if let Some(item) = st.ready.pop_front() {
                return Some((item, st));
            }
            if st.done {
                return None;
            }
            match st.inner.next().await {
                Some(Ok(o)) => {
                    let splitter = match o {
                        LogOutput::StdErr { .. } => &mut st.err,
                        _ => &mut st.out,
                    };
                    let lines = splitter.push(o.as_ref());
                    st.ready.extend(lines.into_iter().map(Ok));
                }
                Some(Err(e)) => st.ready.push_back(Err(DockerError::from(e))),
                None => {
                    st.done = true;
                    let tails = [std::mem::take(&mut st.out), std::mem::take(&mut st.err)];
                    st.ready
                        .extend(tails.into_iter().filter_map(LineSplitter::finish).map(Ok));
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_all(chunks: &[&[u8]]) -> (Vec<String>, Option<String>) {
        let mut s = LineSplitter::default();
        let lines = chunks.iter().flat_map(|c| s.push(c)).collect();
        (lines, s.finish())
    }

    #[test]
    fn line_split_across_chunks_is_joined() {
        assert_eq!(
            push_all(&[b"hel", b"lo\nwor", b"ld\n"]),
            (vec!["hello".into(), "world".into()], None)
        );
    }

    #[test]
    fn several_lines_in_one_chunk() {
        assert_eq!(
            push_all(&[b"a\nb\n\nc\n"]),
            (vec!["a".into(), "b".into(), "".into(), "c".into()], None)
        );
    }

    #[test]
    fn crlf_is_stripped_even_when_split_between_chunks() {
        assert_eq!(
            push_all(&[b"a\r\nb\r", b"\nc\r\n"]),
            (vec!["a".into(), "b".into(), "c".into()], None)
        );
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        assert_eq!(push_all(&[b"\xff\n"]).0, ["\u{fffd}"]);
    }

    #[test]
    fn multibyte_char_split_across_chunks_stays_intact() {
        let bytes = "日本\n".as_bytes();
        assert_eq!(push_all(&[&bytes[..4], &bytes[4..]]).0, ["日本"]);
    }

    #[test]
    fn empty_chunk_changes_nothing() {
        assert_eq!(
            push_all(&[b"", b"a", b"", b"\n", b""]),
            (vec!["a".into()], None)
        );
        assert_eq!(push_all(&[]), (vec![], None));
    }

    #[test]
    fn trailing_partial_line_comes_out_at_finish() {
        assert_eq!(
            push_all(&[b"a\nbc", b"d"]),
            (vec!["a".into()], Some("bcd".into()))
        );
    }

    #[test]
    fn long_line_is_cut_at_cap_and_next_line_is_intact() {
        let mut s = LineSplitter::default();
        let chunk = vec![b'x'; 10 * 1024];
        let mut lines = Vec::new();
        for _ in 0..10 {
            lines.extend(s.push(&chunk));
            assert!(s.buf.len() <= LINE_CAP, "{}", s.buf.len());
        }
        lines.extend(s.push(b"tail\r\nnext\n"));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), LINE_CAP);
        assert!(lines[0].bytes().all(|b| b == b'x'));
        assert_eq!(lines[1], "next");
    }

    #[test]
    fn line_of_exactly_cap_bytes_is_kept_whole() {
        let mut line = vec![b'y'; LINE_CAP];
        line.push(b'\n');
        assert_eq!(
            push_all(&[&line]).0,
            [String::from_utf8(line[..LINE_CAP].to_vec()).unwrap()]
        );
    }

    #[test]
    fn unterminated_huge_tail_is_capped_at_finish() {
        let (lines, tail) = push_all(&[&vec![b'z'; LINE_CAP + 5]]);
        assert!(lines.is_empty());
        assert_eq!(tail.unwrap().len(), LINE_CAP);
    }

    #[test]
    fn cut_line_keeps_crlf_handling_for_following_lines() {
        let mut s = LineSplitter::default();
        let mut big = vec![b'a'; LINE_CAP + 1];
        big.extend_from_slice(b"\r\nb\r\n");
        assert_eq!(s.push(&big).len(), 2);
        assert_eq!(s.push(b"c\r\n"), ["c"]);
    }

    fn out(b: &'static [u8]) -> Result<LogOutput, bollard::errors::Error> {
        Ok(LogOutput::StdOut { message: b.into() })
    }
    fn err(b: &'static [u8]) -> Result<LogOutput, bollard::errors::Error> {
        Ok(LogOutput::StdErr { message: b.into() })
    }

    async fn collect(items: Vec<Result<LogOutput, bollard::errors::Error>>) -> Vec<String> {
        split_log_stream(futures_util::stream::iter(items))
            .map(|r| r.unwrap())
            .collect()
            .await
    }

    #[tokio::test]
    async fn stdout_and_stderr_keep_separate_partial_lines() {
        let v = collect(vec![out(b"o1-"), err(b"e1-"), out(b"o2\n"), err(b"e2\n")]).await;
        assert_eq!(v, ["o1-o2", "e1-e2"]);
    }

    #[tokio::test]
    async fn remainders_are_flushed_at_end_of_stream() {
        let v = collect(vec![out(b"a\nb"), err(b"c")]).await;
        assert_eq!(v, ["a", "b", "c"]);
    }

    #[tokio::test]
    async fn errors_are_passed_on_and_remainder_still_flushes() {
        let items = vec![
            out(b"a"),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 500,
                message: "x".into(),
            }),
            out(b"b\n"),
        ];
        let v: Vec<_> = split_log_stream(futures_util::stream::iter(items))
            .collect()
            .await;
        assert_eq!(v.len(), 2);
        assert!(v[0].is_err());
        assert_eq!(v[1].as_ref().unwrap(), "ab");
    }
}
