//! Decoding of Docker log streams into lines.
//!
//! Without a TTY, Docker multiplexes stdout and stderr into frames with an
//! 8-byte header: `[stream, 0, 0, 0, size (u32 big-endian)]`. With a TTY the
//! stream is raw bytes from stdout only.
//!
//! Docker stores a line longer than 16 KB as several records, and with
//! `timestamps=1` it puts a timestamp in front of every record, not only the
//! first. Only the last record ends with a newline, so read naively the line
//! comes back with a timestamp every 16 KB in the middle of its text. The
//! decoder made by [`LineDecoder::with_timestamps`] joins those records again.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    Stdout,
    Stderr,
}

/// Incremental decoder from raw body chunks to complete lines.
pub struct LineDecoder {
    raw: bool,
    /// Every record starts with Docker's timestamp; see the module docs.
    timestamps: bool,
    frame_buf: Vec<u8>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// How much of a line Docker puts into one record.
const RECORD: usize = 16 * 1024;

impl LineDecoder {
    pub fn new(tty: bool) -> Self {
        Self { raw: tty, timestamps: false, frame_buf: Vec::new(), stdout: Vec::new(), stderr: Vec::new() }
    }

    /// For a log stream read with `timestamps=1`: lines Docker split into
    /// several records come out whole, with only their first timestamp.
    pub fn with_timestamps(tty: bool) -> Self {
        Self { timestamps: true, ..Self::new(tty) }
    }

    /// Feeds a chunk; complete lines are appended to `out` without the trailing newline.
    pub fn push(&mut self, chunk: &[u8], out: &mut Vec<(StreamKind, String)>) {
        let from = out.len();
        if self.raw {
            Self::split(&mut self.stdout, chunk, StreamKind::Stdout, out);
            self.join_tty(out, from);
            return;
        }
        self.frame_buf.extend_from_slice(chunk);
        let mut pos = 0;
        while self.frame_buf.len() - pos >= 8 {
            let h = &self.frame_buf[pos..pos + 8];
            // A header always has three zero bytes; anything else means this is not
            // a multiplexed stream after all (e.g. a TTY we did not know about).
            if h[1] != 0 || h[2] != 0 || h[3] != 0 || h[0] > 2 {
                self.raw = true;
                let rest = self.frame_buf[pos..].to_vec();
                self.frame_buf.clear();
                Self::split(&mut self.stdout, &rest, StreamKind::Stdout, out);
                self.join_tty(out, from);
                return;
            }
            let size = u32::from_be_bytes([h[4], h[5], h[6], h[7]]) as usize;
            if self.frame_buf.len() - pos - 8 < size {
                break;
            }
            let kind = if h[0] == 2 { StreamKind::Stderr } else { StreamKind::Stdout };
            let mut payload = &self.frame_buf[pos + 8..pos + 8 + size];
            let buf = if kind == StreamKind::Stderr { &mut self.stderr } else { &mut self.stdout };
            // One frame is one record. A record that goes on a line the last
            // one left open is the rest of that line: its timestamp goes.
            if self.timestamps && !buf.is_empty() {
                payload = strip_stamp(payload);
            }
            let payload = payload.to_vec();
            Self::split(buf, &payload, kind, out);
            pos += 8 + size;
        }
        self.frame_buf.drain(..pos);
    }

    /// Emits any unterminated trailing data as final lines.
    pub fn finish(&mut self, out: &mut Vec<(StreamKind, String)>) {
        let from = out.len();
        for (buf, kind) in [(&mut self.stdout, StreamKind::Stdout), (&mut self.stderr, StreamKind::Stderr)] {
            if !buf.is_empty() {
                out.push((kind, String::from_utf8_lossy(buf).trim_end_matches('\r').to_string()));
                buf.clear();
            }
        }
        self.join_tty(out, from);
    }

    fn split(buf: &mut Vec<u8>, data: &[u8], kind: StreamKind, out: &mut Vec<(StreamKind, String)>) {
        buf.extend_from_slice(data);
        let mut start = 0;
        while let Some(i) = buf[start..].iter().position(|&b| b == b'\n') {
            let line = &buf[start..start + i];
            out.push((kind, String::from_utf8_lossy(line).trim_end_matches('\r').to_string()));
            start += i + 1;
        }
        buf.drain(..start);
    }

    /// Drops the timestamps Docker put in the middle of a long TTY line.
    /// Without frames the record boundaries are not marked, but every record
    /// holds 16 KB of the line, so the next timestamp is exactly there.
    fn join_tty_records(line: &str) -> String {
        let Some(sp) = line.find(' ') else { return line.to_string() };
        if !is_stamp(&line.as_bytes()[..sp]) || line.len() <= sp + 1 + RECORD {
            return line.to_string();
        }
        let bytes = line.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        out.extend_from_slice(&bytes[..sp + 1]);
        let mut at = sp + 1;
        while bytes.len() - at > RECORD {
            out.extend_from_slice(&bytes[at..at + RECORD]);
            at += RECORD;
            let rest = strip_stamp(&bytes[at..]);
            if rest.len() == bytes.len() - at {
                // Not split here after all: keep the rest as it is.
                break;
            }
            at = bytes.len() - rest.len();
        }
        out.extend_from_slice(&bytes[at..]);
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Joins the long lines among `out[from..]` of a TTY stream read with timestamps.
    fn join_tty(&self, out: &mut [(StreamKind, String)], from: usize) {
        if self.timestamps && self.raw {
            for (_, line) in &mut out[from..] {
                if line.len() > RECORD {
                    *line = Self::join_tty_records(line);
                }
            }
        }
    }
}

/// `2026-10-27T20:15:20.713238298Z`, as Docker writes it.
fn is_stamp(b: &[u8]) -> bool {
    b.len() >= 20
        && b.len() <= 40
        && b[4] == b'-'
        && b[10] == b'T'
        && b.last() == Some(&b'Z')
        && b[..4].iter().all(u8::is_ascii_digit)
        && std::str::from_utf8(b).is_ok_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok())
}

/// The record without its leading `<timestamp> `, or unchanged if it has none.
fn strip_stamp(b: &[u8]) -> &[u8] {
    match b.iter().take(41).position(|&c| c == b' ') {
        Some(sp) if is_stamp(&b[..sp]) => &b[sp + 1..],
        _ => b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(kind: u8, s: &str) -> Vec<u8> {
        let mut v = vec![kind, 0, 0, 0];
        v.extend_from_slice(&(s.len() as u32).to_be_bytes());
        v.extend_from_slice(s.as_bytes());
        v
    }

    #[test]
    fn demuxes_frames_split_across_chunks() {
        let mut data = frame(1, "hello\nwor");
        data.extend(frame(2, "oops\n"));
        data.extend(frame(1, "ld\n"));
        let mut d = LineDecoder::new(false);
        let mut out = vec![];
        for c in data.chunks(3) {
            d.push(c, &mut out);
        }
        assert_eq!(
            out,
            vec![
                (StreamKind::Stdout, "hello".to_string()),
                (StreamKind::Stderr, "oops".to_string()),
                (StreamKind::Stdout, "world".to_string()),
            ]
        );
    }

    /// What Docker sends for one line of `len` bytes with `timestamps=1`:
    /// a record per 16 KB, each with the same timestamp in front.
    fn long_line(c: char, len: usize) -> Vec<String> {
        let text: String = std::iter::repeat_n(c, len).collect();
        let parts: Vec<&str> = text.as_bytes().chunks(RECORD).map(|p| std::str::from_utf8(p).unwrap()).collect();
        let n = parts.len();
        parts.iter().enumerate().map(|(i, p)| format!("2026-11-09T14:11:29.745319828Z {p}{}", if i + 1 == n { "\n" } else { "" })).collect()
    }

    #[test]
    fn joins_the_records_of_a_long_line() {
        let mut data = vec![];
        for r in long_line('a', 40_000) {
            data.extend(frame(1, &r));
        }
        data.extend(frame(1, "2026-11-09T14:11:29.745660915Z short\n"));
        let mut d = LineDecoder::with_timestamps(false);
        let mut out = vec![];
        for c in data.chunks(1000) {
            d.push(c, &mut out);
        }
        assert_eq!(out.len(), 2);
        let (ts, text) = crate::logs::split_docker_timestamp(&out[0].1).unwrap();
        assert_eq!(ts % 1_000_000, 745319);
        assert_eq!(text.len(), 40_000);
        assert!(text.bytes().all(|b| b == b'a'));
        assert!(out[1].1.ends_with(" short"));
    }

    #[test]
    fn joins_the_records_of_a_long_tty_line() {
        let data: String = long_line('b', 40_000).concat() + "2026-11-09T14:11:30.000000000Z next\n";
        let mut d = LineDecoder::with_timestamps(true);
        let mut out = vec![];
        for c in data.as_bytes().chunks(777) {
            d.push(c, &mut out);
        }
        assert_eq!(out.len(), 2);
        let (_, text) = crate::logs::split_docker_timestamp(&out[0].1).unwrap();
        assert_eq!(text.len(), 40_000);
        assert!(text.bytes().all(|b| b == b'b'));
        // A long line that merely contains a date is left alone.
        let mut out = vec![];
        let odd = format!("2026-11-09T14:11:30.000000000Z {}2026-11-09T14:11:30.000000000Z x\n", "c".repeat(RECORD - 5));
        LineDecoder::with_timestamps(true).push(odd.as_bytes(), &mut out);
        assert_eq!(out[0].1, odd.trim_end());
    }

    #[test]
    fn raw_tty_stream() {
        let mut d = LineDecoder::new(true);
        let mut out = vec![];
        d.push(b"a\r\nb", &mut out);
        d.finish(&mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].1, "b");
    }
}
