//! The MQ arithmetic decoder (ISO/IEC 15444-1 Annex C; identical coder to
//! JBIG2). A binary arithmetic decoder driven by a per-context probability
//! state. `decode(cx)` returns one bit (the "decision") and updates that
//! context's state via the standard transition table.

/// One row of the MQ probability estimation state machine (ISO Table C-2):
/// the LPS probability estimate `qe`, the next-state indices on MPS/LPS
/// renormalization, and whether an LPS exchange flips the MPS sense.
#[derive(Debug, Clone, Copy)]
pub struct QeEntry {
    pub qe: u16,
    pub next_mps: u8,
    pub next_lps: u8,
    pub switch: bool,
}

/// The 47-entry MQ state table (ISO Table C-2).
// TODO: fill from ISO/IEC 15444-1 Table C-2 (same constants as the JPEG 2000
// reference and OpenJPEG `mqc_states`).
pub const QE_TABLE: &[QeEntry] = &[];

/// Per-context state: index into [`QE_TABLE`] and the current MPS sense.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context {
    pub index: u8,
    pub mps: u8,
}

/// MQ arithmetic decoder over one code-block's coded bytes.
#[derive(Debug)]
pub struct MqDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    c: u32,
    a: u32,
    ct: i32,
}

impl<'a> MqDecoder<'a> {
    /// Initialise the decoder (INITDEC, ISO C.3.5) over a code-block segment.
    pub fn new(data: &'a [u8]) -> Self {
        let mut d = MqDecoder {
            data,
            pos: 0,
            c: 0,
            a: 0,
            ct: 0,
        };
        d.init();
        d
    }

    fn init(&mut self) {
        todo!("INITDEC: prime BP/C/A/CT and bytein");
    }

    /// Decode one binary decision in context `cx` (DECODE, ISO C.3.2), updating
    /// `cx`'s state. Returns the decoded bit (0 or 1).
    pub fn decode(&mut self, cx: &mut Context) -> u8 {
        todo!("DECODE: A -= Qe; MPS/LPS exchange; renormalize via bytein");
    }
}
