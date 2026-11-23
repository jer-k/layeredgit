use serde::Deserialize;

use crate::{Size, SourceEvent};

const MAX_PENDING: usize = 16 * 1024;

#[derive(Clone, Copy, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Message {
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: Option<u16>,
        cell_height_px: Option<u16>,
    },
    ChildExited {
        exit_code: Option<i64>,
    },
    #[serde(other)]
    Other,
}

pub(super) struct Parser {
    prefix: Vec<u8>,
    pending: Vec<u8>,
}

impl Parser {
    pub(super) fn new(nonce: u64) -> Self {
        Self {
            prefix: format!("\x1b_rex1;{nonce};").into_bytes(),
            pending: Vec::new(),
        }
    }

    pub(super) fn push(&mut self, input: &[u8]) -> Vec<SourceEvent> {
        let mut pending = std::mem::take(&mut self.pending);
        let data = if pending.is_empty() {
            input
        } else {
            pending.extend_from_slice(input);
            &pending
        };
        let mut events = Vec::new();
        let mut output_start = 0;
        let mut cursor = 0;
        while let Some(escape) = data[cursor..].iter().position(|&byte| byte == 0x1b) {
            let start = cursor + escape;
            let rest = &data[start..];
            if self.prefix.starts_with(rest) {
                self.pending.extend_from_slice(rest);
                break;
            }
            cursor = start + 1;
            let Some(body) = rest.strip_prefix(self.prefix.as_slice()) else {
                continue;
            };
            let scan = &body[..body.len().min(MAX_PENDING)];
            let Some(end) = scan.windows(2).position(|pair| pair == b"\x1b\\") else {
                if body.len() < MAX_PENDING {
                    self.pending.extend_from_slice(rest);
                    break;
                }
                continue;
            };
            cursor = start + self.prefix.len() + end + 2;
            if let Ok(message) = serde_json::from_slice(&body[..end]) {
                emit_bytes(&mut events, &data[output_start..start]);
                output_start = cursor;
                if let Some(event) = stream_event(message) {
                    events.push(event);
                }
            }
        }
        emit_bytes(
            &mut events,
            &data[output_start..data.len() - self.pending.len()],
        );
        events
    }

    pub(super) fn flush(&mut self) -> Option<SourceEvent> {
        (!self.pending.is_empty()).then(|| SourceEvent::Output(std::mem::take(&mut self.pending)))
    }
}

fn stream_event(message: Message) -> Option<SourceEvent> {
    match message {
        Message::Resize {
            cols,
            rows,
            cell_width_px,
            cell_height_px,
        } if cols != 0 && rows != 0 => Some(SourceEvent::Resized(Size {
            cols,
            rows,
            cell_width: cell_width_px.unwrap_or(0),
            cell_height: cell_height_px.unwrap_or(0),
        })),
        Message::ChildExited { exit_code } => Some(SourceEvent::Exited(
            exit_code.map_or_else(|| "Rex shell closed".into(), |code| format!("code {code}")),
        )),
        _ => None,
    }
}

fn emit_bytes(events: &mut Vec<SourceEvent>, bytes: &[u8]) {
    if !bytes.is_empty() {
        events.push(SourceEvent::Output(bytes.to_vec()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_apcs_split_at_every_byte_and_preserves_order() {
        let input = b"pre\x1b_rex1;42;{\"type\":\"resize\",\"cols\":80,\"rows\":24,\"owner\":null}\x1b\\middle\x1b_rex1;42;{\"type\":\"theme\"}\x1b\\\x1b_rex1;42;{\"type\":\"unknown\"}\x1b\\\x1b_rex1;42;{\"type\":\"child_exited\",\"exit_code\":7}\x1b\\post";
        for split in 0..=input.len() {
            let mut parser = Parser::new(42);
            let events = parser
                .push(&input[..split])
                .into_iter()
                .chain(parser.push(&input[split..]));
            let mut observed = vec![];
            for event in events {
                match event {
                    SourceEvent::Output(bytes) => observed.extend(bytes),
                    SourceEvent::Resized(size) => {
                        assert_eq!((size.cols, size.rows), (80, 24));
                        observed.push(b'R');
                    }
                    SourceEvent::Exited(reason) => {
                        assert_eq!(reason, "code 7");
                        observed.push(b'E');
                    }
                    other => panic!("Unexpected {other:?}"),
                }
            }
            assert_eq!(observed, b"preRmiddleEpost", "split {split}");
        }
    }

    #[test]
    fn preserves_foreign_and_malformed_sequences() {
        let input =
            b"\xff\x1b_rex1;400;{\"type\":\"theme\"}\x1b\\\x1b_rex1;40;invalid\x1b\\\x1b_other";
        let mut parser = Parser::new(40);
        let bytes: Vec<u8> = input
            .chunks(3)
            .flat_map(|chunk| parser.push(chunk))
            .flat_map(|event| match event {
                SourceEvent::Output(bytes) => bytes,
                other => panic!("Unexpected {other:?}"),
            })
            .collect();
        assert_eq!(bytes, input);
        assert!(parser.flush().is_none());
    }

    #[test]
    fn flushes_unterminated_sequences_and_caps_pending_bytes() {
        let prefix = b"\x1b_rex1;42;";
        let mut parser = Parser::new(42);
        assert!(parser.push(prefix).is_empty());
        assert!(matches!(parser.flush(), Some(SourceEvent::Output(bytes)) if bytes == prefix));
        assert!(parser.push(prefix).is_empty());
        let long = vec![b'a'; MAX_PENDING];
        assert!(
            matches!(parser.push(&long).as_slice(), [SourceEvent::Output(bytes)]
            if bytes.starts_with(prefix) && bytes.ends_with(&long))
        );
        assert!(parser.flush().is_none());
    }
}
