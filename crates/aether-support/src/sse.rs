//! Incremental `text/event-stream` parser (docs/10 §5).
//!
//! Bytes are buffered and only complete lines are decoded, so a multi-byte
//! UTF-8 character split across chunks is never decoded half-way.

/// One dispatched server-sent event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// `event:` field; `None` when absent or empty (the default "message" type).
    pub event: Option<String>,
    /// All `data:` lines of the event joined with `\n`.
    pub data: String,
}

/// Feed raw response chunks in arrival order; each call returns the events completed so far.
#[derive(Debug, Default)]
pub struct SseParser {
    /// Bytes of the current incomplete line.
    buf: Vec<u8>,
    pending: Pending,
}

#[derive(Debug, Default)]
struct Pending {
    event: Option<String>,
    data: Option<String>,
}

impl SseParser {
    /// An empty parser.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Consumes `chunk` and returns every event whose terminating blank line it completed.
    // ponytail: only `\n` / `\r\n` end a line, not a lone `\r`; no target upstream sends one.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        // Only the new bytes can contain a line end; earlier ones were already scanned.
        let mut scan = self.buf.len();
        self.buf.extend_from_slice(chunk);
        let mut events = Vec::new();
        let mut start = 0;
        while let Some(offset) = self.buf[scan..].iter().position(|&byte| byte == b'\n') {
            let end = scan + offset;
            let line = &self.buf[start..end];
            self.pending
                .line(line.strip_suffix(b"\r").unwrap_or(line), &mut events);
            start = end + 1;
            scan = start;
        }
        self.buf.drain(..start);
        events
    }

    /// Ends the stream: a trailing line without newline and an event without its
    /// closing blank line are still delivered, since upstreams often omit them.
    #[must_use]
    pub fn finish(mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        let line = std::mem::take(&mut self.buf);
        let line = line.strip_suffix(b"\r").unwrap_or(&line);
        if !line.is_empty() {
            self.pending.line(line, &mut events);
        }
        self.pending.line(b"", &mut events);
        events
    }
}

impl Pending {
    fn line(&mut self, line: &[u8], events: &mut Vec<SseEvent>) {
        if line.is_empty() {
            let event = self.event.take();
            if let Some(data) = self.data.take() {
                events.push(SseEvent { event, data });
            }
            return;
        }
        let line = String::from_utf8_lossy(line);
        let (field, value) = line.split_once(':').unwrap_or((&line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = (!value.is_empty()).then(|| value.to_owned()),
            "data" => match &mut self.data {
                Some(data) => {
                    data.push('\n');
                    data.push_str(value);
                }
                None => self.data = Some(value.to_owned()),
            },
            // Comments (empty field name), `id`, `retry` and unknown fields carry nothing we use.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "a failed unwrap is a failed test")]
    use std::path::Path;

    use super::*;

    fn parse_in(bytes: &[u8], chunk: usize) -> Vec<SseEvent> {
        let mut parser = SseParser::new();
        let mut events: Vec<SseEvent> = bytes.chunks(chunk).flat_map(|c| parser.feed(c)).collect();
        events.extend(parser.finish());
        events
    }

    fn event(name: Option<&str>, data: &str) -> SseEvent {
        SseEvent {
            event: name.map(str::to_owned),
            data: data.to_owned(),
        }
    }

    #[test]
    fn joins_data_lines_and_ignores_comments_id_retry() {
        let input =
            b": keep-alive\r\nid: 7\r\nretry: 1000\r\nevent: delta\r\ndata: a\r\ndata:b\r\n\r\n\
            event: lonely\n\ndata\n\n";
        assert_eq!(
            parse_in(input, input.len()),
            [event(Some("delta"), "a\nb"), event(None, "")],
            "an event without data is dropped and does not leak its name into the next one"
        );
    }

    #[test]
    fn finish_delivers_an_unterminated_event() {
        assert_eq!(
            parse_in(b"data: {\"x\":1}\r\ndata: tail", 3),
            [event(None, "{\"x\":1}\ntail")]
        );
    }

    #[test]
    fn chinese_split_mid_character_decodes_cleanly() {
        let input = "data: 你好，世界\n\n".as_bytes();
        // Cut inside the 3-byte encoding of '你' (bytes 6..9).
        let mut parser = SseParser::new();
        assert!(parser.feed(&input[..7]).is_empty());
        assert!(parser.feed(&input[7..8]).is_empty());
        assert_eq!(parser.feed(&input[8..]), [event(None, "你好，世界")]);
    }

    /// Every shared fixture yields the same events whether fed whole, in 7-byte or in 1-byte chunks.
    #[test]
    fn fixtures_are_chunking_independent() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/ui/fixtures/sse");
        for protocol in ["anthropic-messages", "openai-chat", "openai-responses"] {
            let mut files: Vec<_> = std::fs::read_dir(root.join(protocol))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
            files.sort();
            assert_eq!(
                files.len(),
                6,
                "{protocol}: docs/12 §3 requires six samples"
            );
            for file in files {
                let bytes = std::fs::read(&file).unwrap();
                let whole = parse_in(&bytes, bytes.len());
                assert!(whole.len() >= 2, "{}", file.display());
                // Samples must stay realistic: every payload is JSON (OpenAI's `[DONE]` aside).
                for e in whole.iter().filter(|e| e.data != "[DONE]") {
                    let parsed = serde_json::from_str::<serde_json::Value>(&e.data);
                    assert!(parsed.is_ok(), "{}: {}", file.display(), e.data);
                }
                assert_eq!(parse_in(&bytes, 7), whole, "{}", file.display());
                assert_eq!(parse_in(&bytes, 1), whole, "{}", file.display());
            }
        }
    }

    #[test]
    fn chinese_fixture_round_trips_text() {
        let bytes = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../packages/ui/fixtures/sse/openai-chat/chinese.sse"),
        )
        .unwrap();
        let text: String = parse_in(&bytes, 1)
            .iter()
            .filter(|e| e.data != "[DONE]")
            .filter_map(|e| {
                let chunk: serde_json::Value = serde_json::from_str(&e.data).unwrap();
                chunk["choices"][0]["delta"]["content"]
                    .as_str()
                    .map(str::to_owned)
            })
            .collect();
        assert_eq!(text, "你好！这是一段中文回复。");
    }
}
