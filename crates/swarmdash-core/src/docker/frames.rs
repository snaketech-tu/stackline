//! Decoding of Docker log streams into lines.
//!
//! Without a TTY, Docker multiplexes stdout and stderr into frames with an
//! 8-byte header: `[stream, 0, 0, 0, size (u32 big-endian)]`. With a TTY the
//! stream is raw bytes from stdout only.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    Stdout,
    Stderr,
}

/// Incremental decoder from raw body chunks to complete lines.
pub struct LineDecoder {
    raw: bool,
    frame_buf: Vec<u8>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl LineDecoder {
    pub fn new(tty: bool) -> Self {
        Self { raw: tty, frame_buf: Vec::new(), stdout: Vec::new(), stderr: Vec::new() }
    }

    /// Feeds a chunk; complete lines are appended to `out` without the trailing newline.
    pub fn push(&mut self, chunk: &[u8], out: &mut Vec<(StreamKind, String)>) {
        if self.raw {
            Self::split(&mut self.stdout, chunk, StreamKind::Stdout, out);
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
                return;
            }
            let size = u32::from_be_bytes([h[4], h[5], h[6], h[7]]) as usize;
            if self.frame_buf.len() - pos - 8 < size {
                break;
            }
            let kind = if h[0] == 2 { StreamKind::Stderr } else { StreamKind::Stdout };
            let payload = self.frame_buf[pos + 8..pos + 8 + size].to_vec();
            let buf = if kind == StreamKind::Stderr { &mut self.stderr } else { &mut self.stdout };
            Self::split(buf, &payload, kind, out);
            pos += 8 + size;
        }
        self.frame_buf.drain(..pos);
    }

    /// Emits any unterminated trailing data as final lines.
    pub fn finish(&mut self, out: &mut Vec<(StreamKind, String)>) {
        for (buf, kind) in [(&mut self.stdout, StreamKind::Stdout), (&mut self.stderr, StreamKind::Stderr)] {
            if !buf.is_empty() {
                out.push((kind, String::from_utf8_lossy(buf).trim_end_matches('\r').to_string()));
                buf.clear();
            }
        }
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
