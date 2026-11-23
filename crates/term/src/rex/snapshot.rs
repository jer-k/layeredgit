use std::io;

use super::socket;

// Ghostty snapshot v1 framing from src/terminal/snapshot/snapshot.ksy upstream.
// libghostty validates the records and CRCs when the UI restores the snapshot.
const MAGIC: &[u8] = b"GHOSTSNP\x01\x00";
const FINISH: u16 = 6;
const RECORD_HEADER: usize = 10;
const MAX_SNAPSHOT: usize = 128 * 1024 * 1024;

pub(super) struct Snapshot {
    data: Vec<u8>,
    offset: usize,
}

impl Snapshot {
    pub(super) fn new() -> Self {
        Self {
            data: Vec::new(),
            offset: MAGIC.len(),
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> io::Result<bool> {
        self.data.extend_from_slice(bytes);
        let prefix = &self.data[..self.data.len().min(MAGIC.len())];
        if !MAGIC.starts_with(prefix) {
            return Err(socket::invalid("Unsupported Rex Ghostty snapshot header"));
        }
        while let Some([tag0, tag1, len0, len1, len2, len3, ..]) =
            self.data.get(self.offset..self.offset + RECORD_HEADER)
        {
            let tag = u16::from_le_bytes([*tag0, *tag1]);
            let length = u32::from_le_bytes([*len0, *len1, *len2, *len3]) as usize;
            let end = self.offset + RECORD_HEADER + length;
            if end > MAX_SNAPSHOT {
                return Err(socket::invalid(
                    "Rex terminal snapshot exceeds the size limit",
                ));
            }
            if end > self.data.len() {
                return Ok(false);
            }
            self.offset = end;
            if tag == FINISH {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn split(mut self) -> (Vec<u8>, Vec<u8>) {
        let remaining = self.data.split_off(self.offset);
        (self.data, remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghostty_vt::{Terminal, TerminalOptions, snapshot};

    #[test]
    fn splits_real_ghostty_snapshot_from_live_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: 80,
            rows: 24,
            ..Default::default()
        })?;
        terminal.vt_write(b"hello");
        let bytes = snapshot::encode(&terminal)?;
        let mut snapshot = Snapshot::new();
        for byte in &bytes[..bytes.len() - 1] {
            assert!(!snapshot.push(&[*byte])?);
        }
        let mut end = bytes[bytes.len() - 1..].to_vec();
        end.extend_from_slice(b"live");
        assert!(snapshot.push(&end)?);
        let (snapshot, live) = snapshot.split();
        assert_eq!(snapshot, &*bytes);
        assert_eq!(live, b"live");
        Ok(())
    }
}
