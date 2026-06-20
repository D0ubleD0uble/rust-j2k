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

#[cfg(test)]
mod tests {
    use super::{QE_TABLE, QeEntry};

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
