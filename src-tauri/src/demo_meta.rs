//! Read what a demo actually says about itself, instead of believing its name.
//!
//! A `.dm_68` filename follows a convention - `map[gametype.physics]mm.ss.mmm
//! (player).dm_68` - and every part of it is guesswork. Old DeFRaG builds wrote
//! the map name wrong (a run of `!minions-run9-strafez!` recorded in 2010 is on
//! disk as `!minions-run9-strafe[...]`, two characters short), files get
//! renamed, and a demo handed over by a third party can say anything at all.
//! The demo itself cannot: it carries the server's own config strings.
//!
//! ## What this reads, and what it deliberately does not
//!
//! A demo is a sequence of `[sequence: i32][length: i32][payload]` blocks, each
//! payload a Huffman-compressed Quake 3 network message. The first message is
//! the gamestate, and a gamestate is laid out config strings first, entity
//! baselines second. Everything worth knowing here - map name, DeFRaG version,
//! game type, whether cheats were on - lives in those config strings.
//!
//! So the reader stops at the first baseline. That boundary is the entire
//! reason this module is short: parsing baselines means delta-compressed entity
//! states, which means the full network field tables, which is where a real
//! demo parser gets big. We never cross it.
//!
//! The Huffman codec (the algorithm and the 256-symbol table) comes from id
//! Software's Quake 3 Arena engine, GPL-2.0. The same table backs the website's
//! Python demo parser; this is an independent Rust reimplementation of the
//! small read-only part of it.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// Anything longer is not a demo message (`MAX_MSGLEN`).
const MESSAGE_MAX_SIZE: i32 = 0x4000;
const MAX_CONFIGSTRINGS: i32 = 1024;
const BIG_INFO_STRING: usize = 8192;

/// Config string 0: what the client was told about the session. `mapname`,
/// `defrag_vers`, `defrag_gametype` and friends live here.
const CS_CLIENT: u16 = 0;
/// Config string 1: the game's own settings, `sv_cheats` among them.
const CS_GAME: u16 = 1;

// Server-to-client message opcodes (`svc_ops_e`).
const SVC_BAD: u8 = 0;
const SVC_NOP: u8 = 1;
const SVC_GAMESTATE: u8 = 2;
const SVC_CONFIGSTRING: u8 = 3;
const SVC_BASELINE: u8 = 4;
const SVC_SERVERCOMMAND: u8 = 5;
const SVC_EOF: u8 = 8;

/// Quake 3 prints `%` as `.` on the wire, and so must we or the strings differ
/// from what every other tool reads.
const PERCENT_CHAR: u8 = 37;
const DOT_CHAR: u8 = 46;

/// The config strings a demo opens with, split into key/value pairs.
pub struct DemoMeta {
    /// Config string 0.
    pub client: HashMap<String, String>,
    /// Config string 1.
    pub game: HashMap<String, String>,
}

impl DemoMeta {
    /// The map this demo was recorded on, as the server named it.
    pub fn map(&self) -> Option<&str> {
        self.client.get("mapname").map(|s| s.as_str()).filter(|s| !s.is_empty())
    }

    /// `sv_cheats` as the server had it. `None` when the demo does not say.
    #[allow(dead_code)]
    pub fn cheats(&self) -> Option<bool> {
        self.game.get("sv_cheats").and_then(|v| v.parse::<i32>().ok()).map(|v| v > 0)
    }

    /// The DeFRaG game type (`defrag_gametype`), when present.
    #[allow(dead_code)]
    pub fn gametype(&self) -> Option<i32> {
        self.client.get("defrag_gametype").and_then(|v| v.parse().ok())
    }
}

/// The map name from inside the demo, or `None` if it cannot be read.
///
/// Never an error the caller has to handle: a truncated, corrupt or simply
/// unfamiliar file just means we do not know, and the caller falls back to the
/// filename it already has.
pub fn map_name(path: &Path) -> Option<String> {
    read(path).ok()?.map().map(|s| s.to_string())
}

/// Read the opening config strings of a demo.
pub fn read(path: &Path) -> Result<DemoMeta, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("Could not open the demo: {e}"))?;

    // The gamestate is the first message. A couple of spare rounds cover a demo
    // that opens with a stray nop or server command, and cap the work at a few
    // kilobytes for a file that is not a demo at all.
    for _ in 0..4 {
        let Some(payload) = next_message(&mut file)? else { break };

        let mut reader = Reader::new(&payload);

        // Every message opens with the reliable-command acknowledgement.
        reader.read_long();

        while !reader.at_end() {
            let Some(command) = reader.read_byte() else { break };

            match command {
                SVC_GAMESTATE => {
                    let (client, game) = read_gamestate(&mut reader);
                    if client.is_some() || game.is_some() {
                        return Ok(DemoMeta {
                            client: client.map(|s| split_config(&s)).unwrap_or_default(),
                            game: game.map(|s| split_config(&s)).unwrap_or_default(),
                        });
                    }
                }
                SVC_SERVERCOMMAND => {
                    reader.read_long();
                    reader.read_string(MAX_STRING_CHARS);
                }
                SVC_BAD | SVC_NOP | SVC_EOF => break,
                // A snapshot or anything else means the gamestate is behind us.
                _ => break,
            }
        }
    }

    Err("This demo does not open with a readable gamestate.".into())
}

const MAX_STRING_CHARS: usize = 1024;

/// One `[sequence][length][payload]` block. `Ok(None)` at the end-of-demo
/// marker or a short read; `Err` only when the file is unreadable.
fn next_message(file: &mut std::fs::File) -> Result<Option<Vec<u8>>, String> {
    let mut header = [0u8; 8];
    if file.read_exact(&mut header).is_err() {
        return Ok(None);
    }

    let sequence = i32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let length = i32::from_le_bytes([header[4], header[5], header[6], header[7]]);

    if sequence == -1 && length == -1 {
        return Ok(None); // the marker a complete demo ends with
    }
    if length <= 0 || length > MESSAGE_MAX_SIZE {
        return Ok(None);
    }

    let mut payload = vec![0u8; length as usize];
    if file.read_exact(&mut payload).is_err() {
        return Ok(None); // truncated: the recording was cut short
    }

    Ok(Some(payload))
}

/// Walk a gamestate's config strings, stopping at the first entity baseline.
///
/// Returns config strings 0 and 1 if they turn up. Anything else is skipped -
/// the map's own string, the players, the sounds - because reading them costs
/// the same walk and nothing here wants them.
fn read_gamestate(reader: &mut Reader) -> (Option<String>, Option<String>) {
    let mut client = None;
    let mut game = None;

    reader.read_long(); // server command sequence

    loop {
        let Some(command) = reader.read_byte() else { break };

        match command {
            SVC_CONFIGSTRING => {
                let Some(index) = reader.read_short() else { break };
                if index as i32 > MAX_CONFIGSTRINGS {
                    break;
                }
                let Some(value) = reader.read_string(BIG_INFO_STRING) else { break };
                match index {
                    CS_CLIENT => client = Some(value),
                    CS_GAME => game = Some(value),
                    _ => {}
                }
                // Both in hand and the rest is of no interest.
                if client.is_some() && game.is_some() {
                    break;
                }
            }
            // Baselines are delta-compressed entity states and reading them
            // needs the whole network field table. Config strings are all
            // written before the first one, so by here we already have
            // everything this module exists for.
            SVC_BASELINE => break,
            _ => break,
        }
    }

    (client, game)
}

/// Split a Quake 3 info string (`\key\value\key\value`) into pairs.
fn split_config(src: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let pieces: Vec<&str> = src.trim_start_matches('\\').split('\\').collect();

    for pair in pieces.chunks(2) {
        if let [key, value] = pair {
            if !key.is_empty() && !value.is_empty() {
                out.insert(key.to_string(), value.to_string());
            }
        }
    }

    out
}

// ---- the Huffman-coded bit stream ------------------------------------------

/// The Quake 3 message Huffman table: for each of the 256 symbols, its code
/// path with a terminating high bit. Walk it from the least significant bit
/// (0 = left, 1 = right) until only the terminator is left.
#[rustfmt::skip]
const SYMTAB: [u16; 256] = [
    0x0006, 0x003B, 0x00C8, 0x00EC, 0x01A1, 0x0111, 0x0090, 0x007F, 0x0035, 0x00B4, 0x00E9, 0x008B, 0x0093,
    0x006D, 0x0139, 0x02AC, 0x00A5, 0x0258, 0x03F0, 0x03F8, 0x05DD, 0x07F3, 0x062B, 0x0723, 0x02F4, 0x058D,
    0x04AB, 0x0763, 0x05EB, 0x0143, 0x024F, 0x01D4, 0x0077, 0x04D3, 0x0244, 0x06CD, 0x07C5, 0x07F9, 0x070D,
    0x07CD, 0x0294, 0x05AC, 0x0433, 0x0414, 0x0671, 0x06F0, 0x03F4, 0x0178, 0x00A7, 0x01C3, 0x01EF, 0x0397,
    0x0153, 0x01B1, 0x020D, 0x0361, 0x0207, 0x02F1, 0x0399, 0x0591, 0x0523, 0x02BC, 0x0344, 0x05F3, 0x01CF,
    0x00D0, 0x00FC, 0x0084, 0x0121, 0x0151, 0x0280, 0x0270, 0x033D, 0x0463, 0x06D7, 0x0771, 0x039D, 0x06AB,
    0x05C7, 0x0733, 0x032C, 0x049D, 0x056B, 0x076B, 0x05D3, 0x0571, 0x05E3, 0x0633, 0x04D7, 0x06CB, 0x0370,
    0x02A8, 0x02C7, 0x0305, 0x02EB, 0x01D8, 0x02F3, 0x013C, 0x03AB, 0x038F, 0x0297, 0x00B0, 0x0141, 0x034F,
    0x005C, 0x0128, 0x02BD, 0x02C4, 0x0198, 0x028F, 0x010C, 0x01B3, 0x0185, 0x018C, 0x0147, 0x0179, 0x00D9,
    0x00C0, 0x0117, 0x0119, 0x014B, 0x01E1, 0x01A3, 0x0173, 0x016F, 0x00E8, 0x0088, 0x00E5, 0x005F, 0x00A9,
    0x00CC, 0x00FD, 0x010F, 0x0183, 0x0101, 0x0187, 0x0167, 0x01E7, 0x0157, 0x0174, 0x03CB, 0x03C4, 0x0281,
    0x024D, 0x0331, 0x0563, 0x0380, 0x07D7, 0x042B, 0x0545, 0x046B, 0x043D, 0x072B, 0x04F9, 0x04E3, 0x0645,
    0x052B, 0x0431, 0x07EB, 0x05B9, 0x0314, 0x05F9, 0x0533, 0x042C, 0x06DD, 0x05C1, 0x071D, 0x05D1, 0x0338,
    0x0461, 0x06E3, 0x0745, 0x066B, 0x04CD, 0x04CB, 0x054D, 0x0238, 0x07C1, 0x063D, 0x07BC, 0x04C5, 0x07AC,
    0x07E3, 0x0699, 0x07D3, 0x0614, 0x0603, 0x05BC, 0x069D, 0x0781, 0x0663, 0x048D, 0x0154, 0x0303, 0x015D,
    0x0060, 0x0089, 0x07C7, 0x0707, 0x01B8, 0x03F1, 0x062C, 0x0445, 0x0403, 0x051D, 0x05C5, 0x074D, 0x041D,
    0x0200, 0x07B9, 0x04DD, 0x0581, 0x050D, 0x04B9, 0x05CD, 0x0794, 0x05BD, 0x0594, 0x078D, 0x0558, 0x07BD,
    0x04C1, 0x07DD, 0x04F8, 0x02D1, 0x0291, 0x0499, 0x06F8, 0x0423, 0x0471, 0x06D3, 0x0791, 0x00C9, 0x0631,
    0x0507, 0x0661, 0x0623, 0x0118, 0x0605, 0x06C1, 0x05D7, 0x04F0, 0x06C5, 0x0700, 0x07D1, 0x07A8, 0x061D,
    0x0D00, 0x0405, 0x0758, 0x06F9, 0x05A8, 0x06B9, 0x068D, 0x00AF, 0x0064,
];

/// A node in the decode tree. Children are indices into the arena, so the tree
/// is one allocation and needs no reference juggling.
#[derive(Clone, Copy)]
struct Node {
    left: Option<u16>,
    right: Option<u16>,
    symbol: Option<u8>,
}

impl Node {
    const EMPTY: Node = Node { left: None, right: None, symbol: None };
}

/// The decode tree, built once from `SYMTAB` and shared by every reader.
fn tree() -> &'static Vec<Node> {
    use std::sync::OnceLock;
    static TREE: OnceLock<Vec<Node>> = OnceLock::new();

    TREE.get_or_init(|| {
        let mut nodes = vec![Node::EMPTY];

        for (symbol, &path) in SYMTAB.iter().enumerate() {
            let mut at = 0usize;
            let mut path = path;

            // The high bit is the terminator, not part of the code.
            while path > 1 {
                let go_right = path & 1 == 1;
                let next = if go_right { nodes[at].right } else { nodes[at].left };

                let next = match next {
                    Some(index) => index as usize,
                    None => {
                        nodes.push(Node::EMPTY);
                        let index = nodes.len() - 1;
                        if go_right {
                            nodes[at].right = Some(index as u16);
                        } else {
                            nodes[at].left = Some(index as u16);
                        }
                        index
                    }
                };

                at = next;
                path >>= 1;
            }

            nodes[at].symbol = Some(symbol as u8);
        }

        nodes
    })
}

/// Reads the Huffman-coded bit stream of one message, least significant bit of
/// each byte first.
struct Reader<'a> {
    data: &'a [u8],
    /// Position in bits.
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn at_end(&self) -> bool {
        self.at >= self.data.len() * 8
    }

    fn next_bit(&mut self) -> Option<u32> {
        if self.at_end() {
            return None;
        }
        let bit = (self.data[self.at / 8] >> (self.at % 8)) & 1;
        self.at += 1;
        Some(bit as u32)
    }

    /// One Huffman symbol, or `None` at the end of the stream.
    fn read_byte(&mut self) -> Option<u8> {
        let nodes = tree();
        let mut at = 0usize;

        loop {
            if let Some(symbol) = nodes[at].symbol {
                return Some(symbol);
            }
            let bit = self.next_bit()?;
            let next = if bit == 0 { nodes[at].left } else { nodes[at].right };
            at = next? as usize;
        }
    }

    /// `bits` bits: any part below a byte boundary comes through raw, the whole
    /// bytes above it come through the Huffman table, low byte first.
    fn read_num_bits(&mut self, bits: usize) -> Option<u32> {
        let fragment = bits & 7;
        let mut value = 0u32;

        if fragment > 0 {
            for shift in 0..fragment {
                value |= self.next_bit()? << shift;
            }
        }

        let whole = bits - fragment;
        if whole > 0 {
            let mut decoded = 0u32;
            for offset in (0..whole).step_by(8) {
                decoded |= (self.read_byte()? as u32) << offset;
            }
            value |= decoded << fragment;
        }

        Some(value)
    }

    fn read_short(&mut self) -> Option<u16> {
        self.read_num_bits(16).map(|v| v as u16)
    }

    fn read_long(&mut self) -> Option<u32> {
        self.read_num_bits(32)
    }

    /// A null-terminated string, with the substitutions Quake 3 makes on the
    /// wire: anything above ASCII, and `%`, becomes `.`.
    fn read_string(&mut self, limit: usize) -> Option<String> {
        let mut out = String::new();

        for _ in 0..limit {
            let byte = self.read_byte()?;
            if byte == 0 {
                break;
            }
            let byte = if byte > 127 || byte == PERCENT_CHAR { DOT_CHAR } else { byte };
            out.push(byte as char);
        }

        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_symbol_decodes_back_to_itself() {
        // Walk each symbol's code path bit by bit and check the tree lands on
        // the symbol it came from. A table typo shows up here and nowhere else.
        for (symbol, &path) in SYMTAB.iter().enumerate() {
            let mut bits: Vec<u8> = Vec::new();
            let mut p = path;
            while p > 1 {
                bits.push((p & 1) as u8);
                p >>= 1;
            }

            // Pack the code into bytes the way the stream carries it.
            let mut data = vec![0u8; bits.len().div_ceil(8)];
            for (i, bit) in bits.iter().enumerate() {
                data[i / 8] |= bit << (i % 8);
            }

            let mut reader = Reader::new(&data);
            assert_eq!(reader.read_byte(), Some(symbol as u8), "symbol {symbol}");
        }
    }

    #[test]
    fn splits_an_info_string() {
        let cfg = split_config("\\mapname\\!minions-run9-strafez!\\defrag_vers\\19116\\empty\\");
        assert_eq!(cfg.get("mapname").map(String::as_str), Some("!minions-run9-strafez!"));
        assert_eq!(cfg.get("defrag_vers").map(String::as_str), Some("19116"));
        assert_eq!(cfg.get("empty"), None);
    }

    #[test]
    fn a_file_that_is_not_a_demo_is_not_an_error_case() {
        let dir = std::env::temp_dir().join("defrag-launcher-demo-meta-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("not-a-demo.dm_68");
        std::fs::write(&path, b"hello, this is not a demo at all").unwrap();

        assert!(map_name(&path).is_none());

        let _ = std::fs::remove_file(&path);
    }
}
