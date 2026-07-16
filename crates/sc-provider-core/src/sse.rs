//! Incremental Server-Sent-Events (SSE) decoding for OpenAI-compatible
//! streaming chat completions.
//!
//! [`SseDecoder`] never buffers a whole HTTP response body. Callers feed
//! it raw bytes as they arrive off the wire via [`SseDecoder::feed`], and
//! it emits [`StreamEvent`]s incrementally as soon as enough bytes have
//! arrived to make sense of them. It correctly handles:
//!
//! - SSE frames (lines, and the blank line that separates events) split
//!   arbitrarily across network chunks,
//! - UTF-8 characters split across chunks - bytes are only ever decoded
//!   once a complete line has been reassembled in our own buffer, never
//!   straight off a raw incoming chunk,
//! - a JSON `data:` payload split across chunks, for the same reason:
//!   JSON is only parsed once its enclosing line is complete,
//! - multi-chunk tool-call deltas, correlated by OpenAI's per-call
//!   `index` field and flushed as one complete [`StreamEvent::ToolUse`]
//!   once finished (the existing [`StreamEvent`] contract has no partial
//!   tool-call variant, so a delta with unparseable-yet JSON arguments
//!   can only ever be surfaced once it is complete),
//! - blank lines (event separators), comment/keep-alive lines (`:...`),
//!   and the `data: [DONE]` terminal marker,
//! - a bounded per-line buffer, so a pathological/never-terminated line
//!   cannot grow memory unboundedly.
//!
//! A single malformed frame, an over-long unterminated line, or invalid
//! UTF-8 permanently poisons the decoder: once it has returned an error,
//! every subsequent [`SseDecoder::feed`] call returns that same error
//! again without processing more input - the stream must be treated as
//! failed and stopped, never silently recovered from, matching the rest
//! of this crate's "never fabricate success after an error" behavior.

use crate::{ProviderError, Result};
use sc_message_types::StreamEvent;
use std::collections::BTreeMap;

/// Default bound on how many bytes an [`SseDecoder`] will buffer for a
/// single, not-yet-terminated line before giving up. Real OpenAI-style
/// chat deltas are at most a few KB; this generous bound only exists to
/// stop a misbehaving (or malicious) server from growing the buffer
/// forever when it never sends a newline.
pub const DEFAULT_MAX_LINE_BUFFER: usize = 256 * 1024;

/// Default bound on the total size of the `data:` payload accumulated
/// for a single SSE event (across all of its `data:` lines, before the
/// blank line that dispatches it). Bounds a server that keeps sending
/// `data:` lines without ever sending the blank line that would flush
/// them.
pub const DEFAULT_MAX_EVENT_PAYLOAD: usize = 1024 * 1024;

/// Default bound on the accumulated `arguments` string for a single
/// in-flight tool call (correlated by its `index`). Bounds a server that
/// keeps sending argument fragments for the same tool call forever.
pub const DEFAULT_MAX_TOOL_CALL_ARGS: usize = 256 * 1024;

#[derive(Debug, Default, Clone)]
struct ToolCallAccumulator {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

#[derive(Debug, serde::Deserialize)]
struct SseChatChunk {
    #[serde(default)]
    choices: Vec<SseChoice>,
}

#[derive(Debug, serde::Deserialize)]
struct SseChoice {
    #[serde(default)]
    delta: Option<SseDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct SseDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<SseToolCallDelta>>,
}

#[derive(Debug, serde::Deserialize)]
struct SseToolCallDelta {
    #[serde(default)]
    index: u64,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<SseFunctionDelta>,
}

#[derive(Debug, serde::Deserialize)]
struct SseFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// Incremental, byte-at-a-time SSE decoder. See module docs.
pub struct SseDecoder {
    max_line_buffer: usize,
    max_event_payload: usize,
    max_tool_call_args: usize,
    line_buffer: Vec<u8>,
    data_lines: Vec<String>,
    /// Running total of bytes pushed into `data_lines` since the last
    /// dispatch, so the `max_event_payload` cap does not require
    /// re-summing `data_lines` on every line.
    event_payload_len: usize,
    tool_calls: BTreeMap<u64, ToolCallAccumulator>,
    poisoned: Option<String>,
}

impl Default for SseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::with_limits(
            DEFAULT_MAX_LINE_BUFFER,
            DEFAULT_MAX_EVENT_PAYLOAD,
            DEFAULT_MAX_TOOL_CALL_ARGS,
        )
    }

    pub fn with_max_line_buffer(max_line_buffer: usize) -> Self {
        Self::with_limits(
            max_line_buffer,
            DEFAULT_MAX_EVENT_PAYLOAD,
            DEFAULT_MAX_TOOL_CALL_ARGS,
        )
    }

    pub fn with_max_event_payload(max_event_payload: usize) -> Self {
        Self::with_limits(
            DEFAULT_MAX_LINE_BUFFER,
            max_event_payload,
            DEFAULT_MAX_TOOL_CALL_ARGS,
        )
    }

    pub fn with_max_tool_call_args(max_tool_call_args: usize) -> Self {
        Self::with_limits(
            DEFAULT_MAX_LINE_BUFFER,
            DEFAULT_MAX_EVENT_PAYLOAD,
            max_tool_call_args,
        )
    }

    pub fn with_limits(
        max_line_buffer: usize,
        max_event_payload: usize,
        max_tool_call_args: usize,
    ) -> Self {
        Self {
            max_line_buffer,
            max_event_payload,
            max_tool_call_args,
            line_buffer: Vec::new(),
            data_lines: Vec::new(),
            event_payload_len: 0,
            tool_calls: BTreeMap::new(),
            poisoned: None,
        }
    }

    /// Feed the next chunk of raw bytes exactly as it arrived off the
    /// wire. Returns zero or more decoded events, in order. Once any
    /// call returns an `Err`, the decoder is poisoned: every future call
    /// (including this one, past the point of failure) returns that same
    /// error without processing further input.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Result<StreamEvent>> {
        if let Some(msg) = self.poisoned.clone() {
            return vec![Err(ProviderError::Stream(msg))];
        }

        self.line_buffer.extend_from_slice(bytes);
        let mut events = Vec::new();

        loop {
            let Some(newline_pos) = self.line_buffer.iter().position(|&b| b == b'\n') else {
                if self.line_buffer.len() > self.max_line_buffer {
                    self.poison_and_report(
                        format!(
                            "SSE line exceeded max buffer size of {} bytes without a newline",
                            self.max_line_buffer
                        ),
                        &mut events,
                    );
                }
                break;
            };

            let mut line_bytes: Vec<u8> = self.line_buffer.drain(..=newline_pos).collect();
            line_bytes.pop(); // drop the '\n'
            if line_bytes.last() == Some(&b'\r') {
                line_bytes.pop(); // tolerate CRLF line endings
            }

            let line = match std::str::from_utf8(&line_bytes) {
                Ok(s) => s.to_string(),
                Err(e) => {
                    self.poison_and_report(
                        format!("SSE line was not valid UTF-8: {e}"),
                        &mut events,
                    );
                    break;
                }
            };

            match self.process_line(&line) {
                Ok(new_events) => events.extend(new_events.into_iter().map(Ok)),
                Err(msg) => {
                    self.poison_and_report(msg, &mut events);
                    break;
                }
            }
        }

        events
    }

    /// Signal end-of-stream: flush any tool call that was still being
    /// accumulated but never got an explicit `finish_reason` (a server
    /// that closes the connection right after its last delta, without a
    /// clean `[DONE]`, is common enough to handle gracefully rather than
    /// dropping the tool call silently).
    pub fn finish(&mut self) -> Vec<Result<StreamEvent>> {
        if let Some(msg) = self.poisoned.clone() {
            return vec![Err(ProviderError::Stream(msg))];
        }
        self.flush_tool_calls().into_iter().map(Ok).collect()
    }

    fn poison_and_report(&mut self, msg: String, events: &mut Vec<Result<StreamEvent>>) {
        self.poisoned = Some(msg.clone());
        events.push(Err(ProviderError::Stream(msg)));
    }

    /// Process one complete, newline-stripped line.
    fn process_line(&mut self, line: &str) -> std::result::Result<Vec<StreamEvent>, String> {
        if line.is_empty() {
            return self.dispatch_event();
        }
        if line.starts_with(':') {
            return Ok(Vec::new()); // comment / keep-alive line
        }
        if let Some(data) = line.strip_prefix("data:") {
            let piece = data.trim_start().to_string();
            self.event_payload_len += piece.len();
            if self.event_payload_len > self.max_event_payload {
                return Err(format!(
                    "SSE event payload exceeded max size of {} bytes without a blank line to dispatch it",
                    self.max_event_payload
                ));
            }
            self.data_lines.push(piece);
            return Ok(Vec::new());
        }
        // Any other SSE field (event:, id:, retry:, ...) is not used by
        // OpenAI-compatible chat streaming. Per the SSE spec, unknown
        // fields are ignored rather than treated as an error.
        Ok(Vec::new())
    }

    /// A blank line dispatches the event accumulated from the `data:`
    /// lines seen since the previous dispatch (per the SSE spec, multiple
    /// `data:` lines in one event are joined with `\n`).
    fn dispatch_event(&mut self) -> std::result::Result<Vec<StreamEvent>, String> {
        if self.data_lines.is_empty() {
            return Ok(Vec::new());
        }
        let payload = self.data_lines.join("\n");
        self.data_lines.clear();
        self.event_payload_len = 0;

        if payload == "[DONE]" {
            let mut events = self.flush_tool_calls();
            events.push(StreamEvent::End {
                finish_reason: None,
            });
            return Ok(events);
        }

        let chunk: SseChatChunk =
            serde_json::from_str(&payload).map_err(|e| format!("malformed SSE data frame: {e}"))?;

        let mut events = Vec::new();
        for choice in chunk.choices {
            if let Some(delta) = &choice.delta {
                if let Some(text) = &delta.content
                    && !text.is_empty()
                {
                    events.push(StreamEvent::TextDelta { text: text.clone() });
                }
                if let Some(tool_calls) = &delta.tool_calls {
                    for tc in tool_calls {
                        let entry = self.tool_calls.entry(tc.index).or_default();
                        if let Some(id) = &tc.id {
                            entry.id = Some(id.clone());
                        }
                        if let Some(function) = &tc.function {
                            if let Some(name) = &function.name {
                                entry.name = Some(name.clone());
                            }
                            if let Some(args) = &function.arguments {
                                if entry.arguments.len() + args.len() > self.max_tool_call_args {
                                    return Err(format!(
                                        "tool call arguments for index {} exceeded max size of {} bytes",
                                        tc.index, self.max_tool_call_args
                                    ));
                                }
                                entry.arguments.push_str(args);
                            }
                        }
                    }
                }
            }
            if let Some(reason) = choice.finish_reason {
                events.extend(self.flush_tool_calls());
                events.push(StreamEvent::End {
                    finish_reason: Some(reason),
                });
            }
        }

        Ok(events)
    }

    fn flush_tool_calls(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        for (_, acc) in std::mem::take(&mut self.tool_calls) {
            let (Some(id), Some(name)) = (acc.id, acc.name) else {
                continue;
            };
            let input = serde_json::from_str(&acc.arguments).unwrap_or(serde_json::Value::Null);
            events.push(StreamEvent::ToolUse { id, name, input });
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_events(mut decoder: SseDecoder, chunks: &[&[u8]]) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        for chunk in chunks {
            for ev in decoder.feed(chunk) {
                out.push(ev.expect("unexpected decoder error"));
            }
        }
        for ev in decoder.finish() {
            out.push(ev.expect("unexpected decoder error on finish"));
        }
        out
    }

    #[test]
    fn single_text_delta_frame() {
        let events = ok_events(
            SseDecoder::new(),
            &[b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n"],
        );

        assert_eq!(
            events,
            vec![StreamEvent::TextDelta {
                text: "hello".into()
            }]
        );
    }

    #[test]
    fn frame_split_arbitrarily_across_chunks() {
        let full = b"data: {\"choices\":[{\"delta\":{\"content\":\"hello world\"}}]}\n\n";
        // Split at every possible byte offset and confirm identical output
        // every time - proves frame reassembly does not depend on where
        // the network happened to cut the bytes.
        for split_at in 0..full.len() {
            let events = ok_events(SseDecoder::new(), &[&full[..split_at], &full[split_at..]]);
            assert_eq!(
                events,
                vec![StreamEvent::TextDelta {
                    text: "hello world".into()
                }],
                "failed with split at byte {split_at}"
            );
        }
    }

    #[test]
    fn utf8_multibyte_character_split_across_chunks() {
        // "café" - the 'é' is a 2-byte UTF-8 sequence (0xC3 0xA9). Split
        // the raw bytes so the cut lands exactly between those two bytes.
        let text = "café";
        let frame = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n");
        let bytes = frame.as_bytes();
        let cut = bytes
            .windows(2)
            .position(|w| w == [0xC3, 0xA9])
            .expect("expected the 2-byte UTF-8 sequence for 'é'")
            + 1;

        let events = ok_events(SseDecoder::new(), &[&bytes[..cut], &bytes[cut..]]);

        assert_eq!(events, vec![StreamEvent::TextDelta { text: text.into() }]);
    }

    #[test]
    fn json_payload_split_mid_object_across_many_chunks() {
        let full = b"data: {\"choices\":[{\"delta\":{\"content\":\"streamed\"}}]}\n\n";
        let mut decoder = SseDecoder::new();
        let mut events = Vec::new();
        // Feed one byte at a time - the most adversarial possible split.
        for byte in full {
            for ev in decoder.feed(std::slice::from_ref(byte)) {
                events.push(ev.unwrap());
            }
        }

        assert_eq!(
            events,
            vec![StreamEvent::TextDelta {
                text: "streamed".into()
            }]
        );
    }

    #[test]
    fn keepalive_comment_lines_are_ignored() {
        let events = ok_events(
            SseDecoder::new(),
            &[
                b": keep-alive\n\n",
                b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
                b": another comment\n\n",
            ],
        );

        assert_eq!(events, vec![StreamEvent::TextDelta { text: "hi".into() }]);
    }

    #[test]
    fn blank_lines_between_events_are_separators_only() {
        let events = ok_events(
            SseDecoder::new(),
            &[
                b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n\n\n",
                b"data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\n",
            ],
        );

        assert_eq!(
            events,
            vec![
                StreamEvent::TextDelta { text: "a".into() },
                StreamEvent::TextDelta { text: "b".into() },
            ]
        );
    }

    #[test]
    fn done_marker_ends_the_stream() {
        let events = ok_events(SseDecoder::new(), &[b"data: [DONE]\n\n"]);

        assert_eq!(
            events,
            vec![StreamEvent::End {
                finish_reason: None
            }]
        );
    }

    #[test]
    fn finish_reason_chunk_emits_end_event() {
        let events = ok_events(
            SseDecoder::new(),
            &[b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n"],
        );

        assert_eq!(
            events,
            vec![StreamEvent::End {
                finish_reason: Some("stop".into())
            }]
        );
    }

    #[test]
    fn tool_call_deltas_are_accumulated_and_flushed_once_complete() {
        let chunks: &[&[u8]] = &[
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]}}]}\n\n",
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\":\"}}]}}]}\n\n",
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"Berlin\\\"}\"}}]}}]}\n\n",
            b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        ];

        let events = ok_events(SseDecoder::new(), chunks);

        assert_eq!(
            events,
            vec![
                StreamEvent::ToolUse {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                    input: serde_json::json!({"city": "Berlin"}),
                },
                StreamEvent::End {
                    finish_reason: Some("tool_calls".into())
                },
            ]
        );
    }

    #[test]
    fn multiple_tool_calls_by_index_do_not_interleave_arguments() {
        let chunks: &[&[u8]] = &[
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"function\":{\"name\":\"fn_a\",\"arguments\":\"{\\\"n\\\":\"}},{\"index\":1,\"id\":\"call_b\",\"function\":{\"name\":\"fn_b\",\"arguments\":\"{\\\"n\\\":\"}}]}}]}\n\n",
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"1}\"}},{\"index\":1,\"function\":{\"arguments\":\"2}\"}}]}}]}\n\n",
            b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        ];

        let events = ok_events(SseDecoder::new(), chunks);

        let mut tool_uses: Vec<_> = events
            .into_iter()
            .filter_map(|e| match e {
                StreamEvent::ToolUse { id, name, input } => Some((id, name, input)),
                _ => None,
            })
            .collect();
        tool_uses.sort_by(|a, b| a.0.cmp(&b.0));

        assert_eq!(
            tool_uses,
            vec![
                (
                    "call_a".to_string(),
                    "fn_a".to_string(),
                    serde_json::json!({"n": 1})
                ),
                (
                    "call_b".to_string(),
                    "fn_b".to_string(),
                    serde_json::json!({"n": 2})
                ),
            ],
            "tool call arguments were interleaved across indices"
        );
    }

    #[test]
    fn oversized_event_payload_without_blank_line_is_bounded_and_errors() {
        // Cap set tiny; feed many `data:` lines (no blank line between
        // them) whose cumulative length exceeds it - simulates a hostile
        // server that never sends the blank line that would flush them.
        let mut decoder = SseDecoder::with_max_event_payload(16);
        let mut events = Vec::new();
        for _ in 0..10 {
            events.extend(decoder.feed(b"data: 0123456789\n"));
        }

        assert!(
            events.iter().any(|e| e.is_err()),
            "expected an error once the cumulative data: payload exceeded the cap"
        );
        match events.iter().find(|e| e.is_err()).unwrap() {
            Err(ProviderError::Stream(msg)) => assert!(msg.contains("event payload")),
            other => panic!("expected a Stream error, got {other:?}"),
        }
    }

    #[test]
    fn oversized_tool_call_arguments_are_bounded_and_error() {
        // Cap set tiny; keep sending argument fragments for the same
        // tool-call index without ever completing it.
        let mut decoder = SseDecoder::with_max_tool_call_args(8);
        let first = decoder.feed(
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"f\",\"arguments\":\"0123\"}}]}}]}\n\n",
        );
        assert!(
            first.is_empty(),
            "first fragment alone must stay under the cap"
        );

        let second = decoder.feed(
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"4567890123\"}}]}}]}\n\n",
        );

        assert_eq!(second.len(), 1);
        match &second[0] {
            Err(ProviderError::Stream(msg)) => assert!(msg.contains("tool call arguments")),
            other => panic!("expected a Stream error, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_frame_is_a_typed_error_and_poisons_decoder() {
        let mut decoder = SseDecoder::new();
        let first = decoder.feed(b"data: { not valid json\n\n");
        assert_eq!(first.len(), 1);
        assert!(first[0].is_err());

        // Poisoned: further input, even well-formed, keeps failing.
        let second = decoder.feed(b"data: {\"choices\":[]}\n\n");
        assert_eq!(second.len(), 1);
        assert!(second[0].is_err());
    }

    #[test]
    fn invalid_utf8_is_a_typed_error() {
        let mut decoder = SseDecoder::new();
        let invalid = vec![b'd', b'a', b't', b'a', b':', b' ', 0xFF, 0xFE, b'\n'];
        let events = decoder.feed(&invalid);
        assert_eq!(events.len(), 1);
        assert!(events[0].is_err());
    }

    #[test]
    fn oversized_unterminated_line_is_bounded_and_errors() {
        let mut decoder = SseDecoder::with_max_line_buffer(16);
        let events = decoder.feed(b"data: this line never terminates and just keeps going");
        assert_eq!(events.len(), 1);
        match &events[0] {
            Err(ProviderError::Stream(msg)) => assert!(msg.contains("max buffer size")),
            other => panic!("expected a Stream error, got {other:?}"),
        }
    }

    #[test]
    fn no_duplicated_text_across_many_arbitrary_split_points() {
        let expected = "The quick brown fox jumps over the lazy dog. 日本語のテスト。";
        let frame = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n",
            expected.replace('"', "\\\"")
        );
        let bytes = frame.as_bytes();

        // A handful of representative split points, including ones that
        // land inside multi-byte UTF-8 sequences.
        for n_parts in [1usize, 2, 3, 5, 7, bytes.len().min(23)] {
            let mut decoder = SseDecoder::new();
            let mut reconstructed = String::new();
            let chunk_size = bytes.len().div_ceil(n_parts).max(1);
            for chunk in bytes.chunks(chunk_size) {
                for ev in decoder.feed(chunk) {
                    if let StreamEvent::TextDelta { text } = ev.unwrap() {
                        reconstructed.push_str(&text);
                    }
                }
            }
            for ev in decoder.finish() {
                if let StreamEvent::TextDelta { text } = ev.unwrap() {
                    reconstructed.push_str(&text);
                }
            }

            assert_eq!(
                reconstructed, expected,
                "text was duplicated or corrupted with {n_parts} chunk(s)"
            );
        }
    }

    #[test]
    fn finish_flushes_tool_call_that_never_got_a_finish_reason() {
        let mut decoder = SseDecoder::new();
        let events = decoder.feed(
            b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_x\",\"function\":{\"name\":\"f\",\"arguments\":\"{}\"}}]}}]}\n\n",
        );
        assert!(events.is_empty());

        let flushed: Vec<_> = decoder.finish().into_iter().map(|e| e.unwrap()).collect();
        assert_eq!(
            flushed,
            vec![StreamEvent::ToolUse {
                id: "call_x".into(),
                name: "f".into(),
                input: serde_json::json!({}),
            }]
        );
    }
}
