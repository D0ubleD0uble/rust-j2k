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

/// The 47-entry MQ state table (ISO/IEC 15444-1 Table C-2), identical to the
/// constants in OpenJPEG's `mqc_states`. Indexed by [`Context::index`]; each row
/// gives the LPS probability `qe`, the next state on an MPS- or LPS-driven
/// renormalization, and whether an LPS exchange flips the MPS sense (`switch`).
#[rustfmt::skip]
pub const QE_TABLE: &[QeEntry] = &[
    QeEntry { qe: 0x5601, next_mps:  1, next_lps:  1, switch: true  },
    QeEntry { qe: 0x3401, next_mps:  2, next_lps:  6, switch: false },
    QeEntry { qe: 0x1801, next_mps:  3, next_lps:  9, switch: false },
    QeEntry { qe: 0x0ac1, next_mps:  4, next_lps: 12, switch: false },
    QeEntry { qe: 0x0521, next_mps:  5, next_lps: 29, switch: false },
    QeEntry { qe: 0x0221, next_mps: 38, next_lps: 33, switch: false },
    QeEntry { qe: 0x5601, next_mps:  7, next_lps:  6, switch: true  },
    QeEntry { qe: 0x5401, next_mps:  8, next_lps: 14, switch: false },
    QeEntry { qe: 0x4801, next_mps:  9, next_lps: 14, switch: false },
    QeEntry { qe: 0x3801, next_mps: 10, next_lps: 14, switch: false },
    QeEntry { qe: 0x3001, next_mps: 11, next_lps: 17, switch: false },
    QeEntry { qe: 0x2401, next_mps: 12, next_lps: 18, switch: false },
    QeEntry { qe: 0x1c01, next_mps: 13, next_lps: 20, switch: false },
    QeEntry { qe: 0x1601, next_mps: 29, next_lps: 21, switch: false },
    QeEntry { qe: 0x5601, next_mps: 15, next_lps: 14, switch: true  },
    QeEntry { qe: 0x5401, next_mps: 16, next_lps: 14, switch: false },
    QeEntry { qe: 0x5101, next_mps: 17, next_lps: 15, switch: false },
    QeEntry { qe: 0x4801, next_mps: 18, next_lps: 16, switch: false },
    QeEntry { qe: 0x3801, next_mps: 19, next_lps: 17, switch: false },
    QeEntry { qe: 0x3401, next_mps: 20, next_lps: 18, switch: false },
    QeEntry { qe: 0x3001, next_mps: 21, next_lps: 19, switch: false },
    QeEntry { qe: 0x2801, next_mps: 22, next_lps: 19, switch: false },
    QeEntry { qe: 0x2401, next_mps: 23, next_lps: 20, switch: false },
    QeEntry { qe: 0x2201, next_mps: 24, next_lps: 21, switch: false },
    QeEntry { qe: 0x1c01, next_mps: 25, next_lps: 22, switch: false },
    QeEntry { qe: 0x1801, next_mps: 26, next_lps: 23, switch: false },
    QeEntry { qe: 0x1601, next_mps: 27, next_lps: 24, switch: false },
    QeEntry { qe: 0x1401, next_mps: 28, next_lps: 25, switch: false },
    QeEntry { qe: 0x1201, next_mps: 29, next_lps: 26, switch: false },
    QeEntry { qe: 0x1101, next_mps: 30, next_lps: 27, switch: false },
    QeEntry { qe: 0x0ac1, next_mps: 31, next_lps: 28, switch: false },
    QeEntry { qe: 0x09c1, next_mps: 32, next_lps: 29, switch: false },
    QeEntry { qe: 0x08a1, next_mps: 33, next_lps: 30, switch: false },
    QeEntry { qe: 0x0521, next_mps: 34, next_lps: 31, switch: false },
    QeEntry { qe: 0x0441, next_mps: 35, next_lps: 32, switch: false },
    QeEntry { qe: 0x02a1, next_mps: 36, next_lps: 33, switch: false },
    QeEntry { qe: 0x0221, next_mps: 37, next_lps: 34, switch: false },
    QeEntry { qe: 0x0141, next_mps: 38, next_lps: 35, switch: false },
    QeEntry { qe: 0x0111, next_mps: 39, next_lps: 36, switch: false },
    QeEntry { qe: 0x0085, next_mps: 40, next_lps: 37, switch: false },
    QeEntry { qe: 0x0049, next_mps: 41, next_lps: 38, switch: false },
    QeEntry { qe: 0x0025, next_mps: 42, next_lps: 39, switch: false },
    QeEntry { qe: 0x0015, next_mps: 43, next_lps: 40, switch: false },
    QeEntry { qe: 0x0009, next_mps: 44, next_lps: 41, switch: false },
    QeEntry { qe: 0x0005, next_mps: 45, next_lps: 42, switch: false },
    QeEntry { qe: 0x0001, next_mps: 45, next_lps: 43, switch: false },
    QeEntry { qe: 0x5601, next_mps: 46, next_lps: 46, switch: false },
];

/// Per-context state: index into [`QE_TABLE`] and the current MPS sense.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context {
    pub index: u8,
    pub mps: u8,
}

/// MQ arithmetic decoder over one code-block's coded bytes.
///
/// Implements the ISO/IEC 15444-1 Annex C "hardware" convention directly:
/// `c`/`a` are the code and interval registers (the comparison reads the high
/// 16 bits of `c`, `c >> 16`), `ct` counts the bits left before the next
/// [`bytein`](Self::bytein), and `pos` is the byte pointer `BP`.
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

    /// The coded byte at `pos`, or `0xFF` once `pos` runs off the end. Past the
    /// segment the decoder is fed 1-bits — the marker convention of ISO C.3.4
    /// (a terminating marker reads as `0xFF`) — which also bounds every read so
    /// a truncated or exhausted segment can never index out of `data`.
    fn byte(&self, pos: usize) -> u8 {
        self.data.get(pos).copied().unwrap_or(0xFF)
    }

    /// INITDEC (ISO C.3.5): prime the registers and fold in the first byte.
    fn init(&mut self) {
        self.pos = 0;
        self.c = (self.byte(self.pos) as u32) << 16;
        self.bytein();
        self.c <<= 7;
        self.ct -= 7;
        self.a = 0x8000;
    }

    /// BYTEIN (ISO C.3.4): pull the next coded byte into `c`, handling the
    /// `0xFF` stuffing carry (a stuffed byte contributes seven bits, not eight,
    /// and a `0xFF` followed by `> 0x8F` is a marker, so no byte is consumed).
    fn bytein(&mut self) {
        if self.byte(self.pos) == 0xFF {
            if self.byte(self.pos + 1) > 0x8F {
                self.c += 0xFF00;
                self.ct = 8;
            } else {
                self.pos += 1;
                self.c += (self.byte(self.pos) as u32) << 9;
                self.ct = 7;
            }
        } else {
            self.pos += 1;
            self.c += (self.byte(self.pos) as u32) << 8;
            self.ct = 8;
        }
    }

    /// RENORMD (ISO C.3.3): shift `a`/`c` left until `a` is renormalized,
    /// pulling a fresh byte whenever the bit count runs out.
    fn renormd(&mut self) {
        loop {
            if self.ct == 0 {
                self.bytein();
            }
            self.a <<= 1;
            self.c <<= 1;
            self.ct -= 1;
            if self.a & 0x8000 != 0 {
                break;
            }
        }
    }

    /// MPS_EXCHANGE (ISO C.16): decide and re-estimate when the MPS sub-interval
    /// is the smaller one. `qe` is the current context's `Qe`.
    fn mps_exchange(&mut self, cx: &mut Context, qe: u32) -> u8 {
        let e = QE_TABLE[cx.index as usize];
        if self.a < qe {
            let d = 1 - cx.mps;
            if e.switch {
                cx.mps = 1 - cx.mps;
            }
            cx.index = e.next_lps;
            d
        } else {
            cx.index = e.next_mps;
            cx.mps
        }
    }

    /// LPS_EXCHANGE (ISO C.17): decide and re-estimate on the LPS path, then set
    /// `a` to the LPS sub-interval `qe`.
    fn lps_exchange(&mut self, cx: &mut Context, qe: u32) -> u8 {
        let e = QE_TABLE[cx.index as usize];
        let d = if self.a < qe {
            cx.index = e.next_mps;
            cx.mps
        } else {
            let d = 1 - cx.mps;
            if e.switch {
                cx.mps = 1 - cx.mps;
            }
            cx.index = e.next_lps;
            d
        };
        self.a = qe;
        d
    }

    /// Decode one binary decision in context `cx` (DECODE, ISO C.3.2), updating
    /// `cx`'s state. Returns the decoded bit (0 or 1).
    pub fn decode(&mut self, cx: &mut Context) -> u8 {
        let qe = QE_TABLE[cx.index as usize].qe as u32;
        self.a -= qe;
        if (self.c >> 16) < qe {
            let d = self.lps_exchange(cx, qe);
            self.renormd();
            d
        } else {
            self.c -= qe << 16;
            if self.a & 0x8000 == 0 {
                let d = self.mps_exchange(cx, qe);
                self.renormd();
                d
            } else {
                cx.mps
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Context, MqDecoder, QE_TABLE, QeEntry};

    /// ISO/IEC 15444-1 Annex C / ITU-T T.88 (08/2018) §H.2 "Test sequence for
    /// arithmetic coder": these 30 coded bytes decode to [`H2_DECODED`] under a
    /// single context that starts at state 0 with MPS 0. It is the standard's
    /// own worked example, so it proves the decoder against an external oracle
    /// rather than against our own output.
    #[rustfmt::skip]
    const H2_CODED: [u8; 30] = [
        0x84, 0xC7, 0x3B, 0xFC, 0xE1, 0xA1, 0x43, 0x04,
        0x02, 0x20, 0x00, 0x00, 0x41, 0x0D, 0xBB, 0x86,
        0xF4, 0x31, 0x7F, 0xFF, 0x88, 0xFF, 0x37, 0x47,
        0x1A, 0xDB, 0x6A, 0xDF, 0xFF, 0xAC,
    ];

    /// The 256 decisions H.2 expects, packed MSB-first into 32 bytes.
    #[rustfmt::skip]
    const H2_DECODED: [u8; 32] = [
        0x00, 0x02, 0x00, 0x51, 0x00, 0x00, 0x00, 0xC0,
        0x03, 0x52, 0x87, 0x2A, 0xAA, 0xAA, 0xAA, 0xAA,
        0x82, 0xC0, 0x20, 0x00, 0xFC, 0xD7, 0x9E, 0xF6,
        0xBF, 0x7F, 0xED, 0x90, 0x4F, 0x46, 0xA3, 0xBF,
    ];

    /// The worked test sequence decodes to the exact expected decision stream.
    #[test]
    fn h2_test_sequence_decodes_exactly() {
        let mut mq = MqDecoder::new(&H2_CODED);
        // "For this entire test, a single value of CX is used. I(CX) is
        // initially 0 and MPS(CX) is initially 0." (H.2)
        let mut cx = Context::default();
        for (i, &expected) in H2_DECODED.iter().enumerate() {
            let mut actual = 0u8;
            for _ in 0..8 {
                actual = (actual << 1) | mq.decode(&mut cx);
            }
            assert_eq!(actual, expected, "decoded byte {i}");
        }
    }

    /// Decoding past the end of the segment is well defined (the marker
    /// convention feeds 1-bits) and must never read past `data` or panic.
    #[test]
    fn decode_past_end_is_bounded() {
        let mut mq = MqDecoder::new(&H2_CODED);
        let mut cx = Context::default();
        // Far more decisions than the segment encodes; the guard in `byte`
        // keeps every read in bounds.
        for _ in 0..(H2_DECODED.len() * 8 * 4) {
            let _ = mq.decode(&mut cx);
        }
    }

    /// A tiny and an empty segment must initialise and decode without panicking
    /// (every read falls through to the `0xFF` marker convention).
    #[test]
    fn tiny_and_empty_segments_do_not_panic() {
        for data in [[].as_slice(), [0xFF].as_slice(), [0x00].as_slice()] {
            let mut mq = MqDecoder::new(data);
            let mut cx = Context::default();
            for _ in 0..64 {
                let _ = mq.decode(&mut cx);
            }
        }
    }

    /// ISO/IEC 15444-1 Table C-2, transcribed independently of [`QE_TABLE`] as
    /// `(qe, next_mps, next_lps, switch)`. Kept as a separate literal so a
    /// single mistyped cell in either copy is caught by `table_matches_iso`.
    #[rustfmt::skip]
    const ISO_TABLE_C2: [(u16, u8, u8, bool); 47] = [
        (0x5601,  1,  1, true),  (0x3401,  2,  6, false), (0x1801,  3,  9, false),
        (0x0ac1,  4, 12, false), (0x0521,  5, 29, false), (0x0221, 38, 33, false),
        (0x5601,  7,  6, true),  (0x5401,  8, 14, false), (0x4801,  9, 14, false),
        (0x3801, 10, 14, false), (0x3001, 11, 17, false), (0x2401, 12, 18, false),
        (0x1c01, 13, 20, false), (0x1601, 29, 21, false), (0x5601, 15, 14, true),
        (0x5401, 16, 14, false), (0x5101, 17, 15, false), (0x4801, 18, 16, false),
        (0x3801, 19, 17, false), (0x3401, 20, 18, false), (0x3001, 21, 19, false),
        (0x2801, 22, 19, false), (0x2401, 23, 20, false), (0x2201, 24, 21, false),
        (0x1c01, 25, 22, false), (0x1801, 26, 23, false), (0x1601, 27, 24, false),
        (0x1401, 28, 25, false), (0x1201, 29, 26, false), (0x1101, 30, 27, false),
        (0x0ac1, 31, 28, false), (0x09c1, 32, 29, false), (0x08a1, 33, 30, false),
        (0x0521, 34, 31, false), (0x0441, 35, 32, false), (0x02a1, 36, 33, false),
        (0x0221, 37, 34, false), (0x0141, 38, 35, false), (0x0111, 39, 36, false),
        (0x0085, 40, 37, false), (0x0049, 41, 38, false), (0x0025, 42, 39, false),
        (0x0015, 43, 40, false), (0x0009, 44, 41, false), (0x0005, 45, 42, false),
        (0x0001, 45, 43, false), (0x5601, 46, 46, false),
    ];

    #[test]
    fn table_has_47_states() {
        assert_eq!(QE_TABLE.len(), 47);
    }

    #[test]
    fn anchor_state_zero() {
        // The fixed start state every context is initialised to (ISO C.3.4).
        let e = QE_TABLE[0];
        assert_eq!(e.qe, 0x5601);
        assert_eq!(e.next_mps, 1);
        assert_eq!(e.next_lps, 1);
        assert!(e.switch);
    }

    #[test]
    fn table_matches_iso() {
        for (i, &(qe, next_mps, next_lps, switch)) in ISO_TABLE_C2.iter().enumerate() {
            let QeEntry {
                qe: gqe,
                next_mps: gmps,
                next_lps: glps,
                switch: gsw,
            } = QE_TABLE[i];
            assert_eq!(
                (gqe, gmps, glps, gsw),
                (qe, next_mps, next_lps, switch),
                "row {i}"
            );
        }
    }

    #[test]
    fn transitions_stay_in_bounds() {
        let n = QE_TABLE.len() as u8;
        for (i, e) in QE_TABLE.iter().enumerate() {
            assert!(
                e.next_mps < n,
                "row {i}: next_mps {} out of range",
                e.next_mps
            );
            assert!(
                e.next_lps < n,
                "row {i}: next_lps {} out of range",
                e.next_lps
            );
        }
    }

    /// Structural facts of Table C-2 that hold independently of the row-by-row
    /// values, so they catch a *systematic* transcription error that the twin
    /// `ISO_TABLE_C2` (derived the same way) would share and miss.
    #[test]
    fn known_structural_invariants() {
        // Exactly three states flip the MPS sense on an LPS exchange, and they
        // are states 0, 6 and 14 (the three Qe = 0x5601 "restart" rows).
        let switches: Vec<usize> = QE_TABLE
            .iter()
            .enumerate()
            .filter(|(_, e)| e.switch)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(switches, vec![0, 6, 14]);
        assert!(switches.iter().all(|&i| QE_TABLE[i].qe == 0x5601));

        // The least-probable terminal state is absorbing: both transitions stay.
        let last = QE_TABLE[46];
        assert_eq!(last.next_mps, 46);
        assert_eq!(last.next_lps, 46);
    }
}
