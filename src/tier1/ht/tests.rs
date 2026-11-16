use super::*;
use crate::tier2::{CodeBlock, CodedSegment};

/// A one-pass HT code-block of `width` x `height` with `zbp` zero bit-planes
/// whose cleanup segment is `bytes`.
fn block(width: usize, height: usize, zbp: u32, bytes: &[u8]) -> CodeBlock<'_> {
    CodeBlock {
        x: 0,
        y: 0,
        width,
        height,
        num_passes: 1,
        zero_bit_planes: zbp,
        segments: vec![CodedSegment {
            passes: 1,
            chunks: vec![bytes],
        }],
        cut: false,
        placeholders: 0,
        coder: crate::tier2::Coder::Ht,
        missing_from: None,
    }
}

/// The parameters of a block under a maxshift of `roi_shift`, with no `CAP`
/// bound.
fn params(roi_shift: u8, causal: bool) -> Params {
    Params {
        roi_shift,
        causal,
        bound_bits: None,
    }
}

/// The `Codestream` message of a decode that must fail as malformed.
fn malformed_msg(block: &CodeBlock<'_>, numbps: u32) -> String {
    match decode(block, numbps, params(0, false)) {
        Err(Error::Codestream(msg)) => msg,
        other => panic!("expected a Codestream error, got {other:?}"),
    }
}

/// A small linear congruential generator, so the randomized test needs no
/// dependency and repeats exactly.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

// The tables.

/// FNV-1a 64 over every field of every row, as `checksum()` in
/// `scripts/gen-ht-vlc-tables.py` computes it.
fn checksum() -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for row in CXT_VLC_TABLE_0.iter().chain(&CXT_VLC_TABLE_1) {
        for &v in row {
            h ^= u64::from(v);
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
    h
}

#[test]
fn the_vlc_tables_match_the_generator_checksum() {
    assert_eq!(CXT_VLC_TABLE_0.len(), 444);
    assert_eq!(CXT_VLC_TABLE_1.len(), 358);
    assert_eq!(checksum(), 0x6087_F933_13D5_93F0);
}

/// Each context's codewords form a complete prefix code: no codeword is a
/// prefix of another, and their Kraft sum is exactly 1, so every bit string
/// of 7 bits starts with exactly one codeword.
#[test]
fn each_vlc_context_is_a_complete_prefix_code() {
    for (name, table) in [("0", &CXT_VLC_TABLE_0[..]), ("1", &CXT_VLC_TABLE_1[..])] {
        for c in 0..8u8 {
            let rows: Vec<_> = table.iter().filter(|r| r[0] == c).collect();
            assert!(!rows.is_empty(), "table {name} context {c} is empty");
            // In units of 2^-7.
            let kraft: u32 = rows.iter().map(|r| 1 << (7 - r[6])).sum();
            assert_eq!(kraft, 128, "table {name} context {c} Kraft sum");
            for a in &rows {
                assert!((1..=7).contains(&a[6]) && u32::from(a[5]) < 1 << a[6]);
                for b in &rows {
                    let short = a[6].min(b[6]);
                    let mask = (1u8 << short) - 1;
                    if !std::ptr::eq(*a, *b) {
                        assert_ne!(a[5] & mask, b[5] & mask, "table {name} context {c}");
                    }
                }
            }
        }
    }
}

/// Reading each codeword through `decode_cxt_vlc` returns its own row.
#[test]
fn each_codeword_decodes_to_its_row() {
    for (first_row, table) in [(true, &CXT_VLC_TABLE_0[..]), (false, &CXT_VLC_TABLE_1[..])] {
        for row in table {
            let [c, rho, u_off, e_k, e_1, w, _] = *row;
            // The VLC stream starts at the high nibble of byte `Lcup - 2`
            // (0x0 here, so four bits), then reads byte `Lcup - 3` LSB first.
            let data = [w, 0x02, 0x00];
            let seg = Segment {
                data: &data,
                pcup: 0,
            };
            let mut vlc = VlcReader::new(&seg);
            assert_eq!(vlc.bits(&seg, 4).unwrap(), 0);
            let got = decode_cxt_vlc(&mut vlc, &seg, first_row, c).unwrap();
            let want = QuadInfo {
                rho,
                u_off: u_off == 1,
                e_k,
                e_1,
            };
            assert_eq!(got, want, "row {row:?}");
        }
    }
}

// The three bit-streams.

#[test]
fn magsgn_reads_lsb_first_skips_stuffing_and_implies_one_ff() {
    let data = [0xFF, 0x7F, 0x00, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 2,
    };
    let mut ms = MagSgnReader::new();
    let bits: Vec<u32> = (0..23).map(|_| ms.bit(&seg).unwrap()).collect();
    // 0xFF, then seven bits of 0x7F (its top bit is the stuffing bit), then
    // all eight bits of the 0xFF implied at Pcup.
    let mut want = vec![1; 8];
    want.extend([1; 7]);
    want.extend([1; 8]);
    assert_eq!(bits, want);
    assert!(matches!(ms.bit(&seg), Err(Error::Codestream(_))));
}

#[test]
fn magsgn_value_is_lsb_first() {
    let data = [0b1011_0010, 0x00, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 1,
    };
    let mut ms = MagSgnReader::new();
    assert_eq!(ms.value(&seg, 3).unwrap(), 0b010);
    assert_eq!(ms.value(&seg, 5).unwrap(), 0b10110);
}

#[test]
fn mel_bits_are_msb_first_with_stuffing() {
    let data = [0xFF, 0x40, 0x00, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 0,
    };
    let mut mel = MelDecoder::new(0);
    let bits: Vec<u32> = (0..15).map(|_| mel.bit(&seg)).collect();
    // Eight ones, then the seven low bits of 0x40.
    let mut want = vec![1; 8];
    want.extend([1, 0, 0, 0, 0, 0, 0]);
    assert_eq!(bits, want);
}

/// MEL reads byte `Lcup - 2` with its `Scup` nibble as ones, and the last
/// byte as 0xFF (`modDcup`).
#[test]
fn mel_reads_the_scup_bytes_as_ones() {
    let data = [0x02, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 0,
    };
    let mut mel = MelDecoder::new(0);
    let bits: Vec<u32> = (0..16).map(|_| mel.bit(&seg)).collect();
    let mut want = vec![0, 0, 0, 0, 1, 1, 1, 1];
    want.extend([1; 8]);
    assert_eq!(bits, want);
}

#[test]
fn mel_symbols_follow_the_run_length_states() {
    // Bits 1111 0110, then zeros. States 0..2 (exponent 0) turn each 1 into
    // one zero symbol; state 3 (exponent 1) turns a 1 into two; a 0 in state
    // 4 reads one more bit as the run before a one symbol and drops a state.
    let data = [0xF6, 0x00, 0x00, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 0,
    };
    let mut mel = MelDecoder::new(0);
    let syms: Vec<u32> = (0..10).map(|_| mel.symbol(&seg)).collect();
    assert_eq!(syms, [0, 0, 0, 0, 0, 0, 1, 0, 0, 1]);
}

#[test]
fn mel_past_the_end_reads_ones() {
    let data = [0x00, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 2,
    };
    let mut mel = MelDecoder::new(2);
    assert!((0..1000).all(|_| mel.symbol(&seg) == 0));
    assert_eq!(mel.k, 12);
}

#[test]
fn vlc_starts_with_three_bits_after_a_seven_and_skips_stuffing() {
    // modDcup turns byte Lcup - 2 into 0xFF: its nibble 0xF starts the
    // stream with three bits, and 0x7F after it carries a stuffing bit.
    let data = [0x7F, 0xF2, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 0,
    };
    let mut vlc = VlcReader::new(&seg);
    assert_eq!(vlc.bits(&seg, 10).unwrap(), 0x3FF);
    assert!(matches!(vlc.bit(&seg), Err(Error::Codestream(_))));

    // Byte Lcup - 2 reads as 0x7F: three bits, then all eight of 0x7F.
    let data = [0x7F, 0x72, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 0,
    };
    let mut vlc = VlcReader::new(&seg);
    assert_eq!(vlc.bits(&seg, 11).unwrap(), 0x3FF);
    assert!(matches!(vlc.bit(&seg), Err(Error::Codestream(_))));
}

/// A set stuffing bit is skipped, not read (7.1.4): the next byte's bits
/// start where it would have been. OpenJPEG ORs it into the next byte's
/// lowest bit instead.
#[test]
fn vlc_skips_a_set_stuffing_bit() {
    // The first nibble 0xF gives three bits and drops its top bit.
    let data = [0x00, 0xF2, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 0,
    };
    let mut vlc = VlcReader::new(&seg);
    assert_eq!(vlc.bits(&seg, 11).unwrap(), 0x7);
    // 0xFF after 0x9F gives seven bits and drops its top bit.
    let data = [0x00, 0xFF, 0x92, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 0,
    };
    let mut vlc = VlcReader::new(&seg);
    assert_eq!(vlc.bits(&seg, 12).unwrap(), 0x7F9);
}

/// A 2x8 block whose VLC stream starts with the nibble 0xF: with its
/// stuffing bit cleared (0x7) it decodes the same. OpenJPEG 2.5.0 decodes
/// the 0x7 block as this does and the 0xF block differently.
#[test]
fn a_set_vlc_stuffing_bit_does_not_change_the_block() {
    let set = [0x7F, 0x47, 0x93, 0xF7, 0x5C, 0xF6, 0x00];
    let mut clear = set;
    clear[5] = 0x76;
    let a = decode(&block(2, 8, 2, &set), 6, params(0, false)).unwrap();
    assert!(a.iter().any(|&c| c != 0));
    assert_eq!(
        a,
        decode(&block(2, 8, 2, &clear), 6, params(0, false)).unwrap()
    );
}

/// A 0x7F byte after 0x8F keeps all eight bits: only a byte above 0x8F
/// makes the next one's top bit a stuffing bit.
#[test]
fn vlc_does_not_stuff_after_0x8f() {
    let data = [0x7F, 0x8F, 0x02, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 0,
    };
    let mut vlc = VlcReader::new(&seg);
    assert_eq!(vlc.bits(&seg, 4).unwrap(), 0);
    assert_eq!(vlc.bits(&seg, 8).unwrap(), 0x8F);
    assert_eq!(vlc.bits(&seg, 8).unwrap(), 0x7F);
    assert!(matches!(vlc.bit(&seg), Err(Error::Codestream(_))));
}

#[test]
fn vlc_starts_with_four_bits_below_a_seven() {
    let data = [0xAA, 0x62, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 0,
    };
    let mut vlc = VlcReader::new(&seg);
    assert_eq!(vlc.bits(&seg, 4).unwrap(), 0x6);
    assert_eq!(vlc.bits(&seg, 8).unwrap(), 0xAA);
}

#[test]
fn u_vlc_prefix_suffix_and_extension() {
    // Bits, LSB first from the nibble 0x0 then byte 0: four zeros, then the
    // byte's bits.
    let cases: [(u8, u32, u32, u32); 4] = [
        // 1 -> prefix 1.
        (0b0000_0001, 1, 0, 0),
        // 01 -> prefix 2.
        (0b0000_0010, 2, 0, 0),
        // 001, suffix bit 1 -> prefix 3, suffix 1.
        (0b0000_1100, 3, 1, 0),
        // 000, five suffix bits 11111 (28 or more), then 0x05 gives the extension 0101.
        (0b1111_1000, 5, 31, 0b0101),
    ];
    for (byte, prefix, suffix, ext) in cases {
        let data = [0x05, byte, 0x02, 0x00];
        let seg = Segment {
            data: &data,
            pcup: 0,
        };
        let mut vlc = VlcReader::new(&seg);
        assert_eq!(vlc.bits(&seg, 4).unwrap(), 0);
        let p = vlc.u_prefix(&seg).unwrap();
        let s = vlc.u_suffix(&seg, p).unwrap();
        let e = vlc.u_extension(&seg, s).unwrap();
        assert_eq!((p, s, e), (prefix, suffix, ext), "byte {byte:#010b}");
    }
}

// Quad steps.

/// A cleanup pass over `seg` for a `width` x `height` block.
fn cleanup<'a>(seg: Segment<'a>, width: usize, height: usize) -> Cleanup<'a> {
    Cleanup {
        seg,
        magsgn: MagSgnReader::new(),
        mel: MelDecoder::new(seg.pcup),
        vlc: VlcReader::new(&seg),
        width,
        height,
        qw: width.div_ceil(2),
        max_u: 30,
        bound_bits: None,
        shift: 0,
    }
}

/// In the first row, a quad-pair with both residuals and MEL symbol 0 reads
/// a full U-VLC prefix for the second quad unless the first prefix is over
/// 2, when one bit picks prefix 1 or 2.
#[test]
fn a_first_row_pair_reads_its_second_prefix_by_the_first() {
    let both = [QuadInfo {
        u_off: true,
        ..QuadInfo::default()
    }; 2];
    // MEL byte 0x80: symbol 0. VLC nibble 0x6, LSB first 0, 1, 1: prefix 2
    // for the first quad, then prefix 1 for the second.
    let data = [0x80, 0x63, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 0,
    };
    assert_eq!(cleanup(seg, 4, 2).residuals(true, &both).unwrap(), [2, 1]);
    // VLC nibble 0xC, LSB first 0, 0, 1, 1: prefix 3, then one bit 1 for
    // prefix 2. The first quad's suffix bit is the low bit of byte 0, 0.
    let data = [0x80, 0xC3, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 0,
    };
    assert_eq!(cleanup(seg, 4, 2).residuals(true, &both).unwrap(), [3, 2]);
}

/// The predictor takes the largest exponent of the samples above the quad:
/// both of the quad above, and one each from the quads above left and above
/// right, where they exist.
#[test]
fn the_predictor_reads_the_quads_above() {
    let data = [0x00, 0x02, 0x00];
    let seg = Segment {
        data: &data,
        pcup: 0,
    };
    let c = cleanup(seg, 4, 4);
    // Two quads per row. Above quad 2 (first column) is quad 0, and quad 1
    // is above right: its bottom-left sample is index 5.
    let mut exps = vec![0u32; 16];
    exps[5] = 10;
    assert_eq!(c.predictor(&exps, 2, 0b11), 9);
    // Quad 3 (last column) has no quad above right; index 9 is quad 2.
    exps[9] = 10;
    exps[5] = 0;
    assert_eq!(c.predictor(&exps, 3, 0b11), 1);
    // Above left of quad 3 is quad 0: its bottom-right sample is index 3.
    exps[3] = 7;
    assert_eq!(c.predictor(&exps, 3, 0b11), 6);
    // One significant sample: no prediction.
    assert_eq!(c.predictor(&exps, 3, 0b01), 1);
}

// Whole blocks.

/// One sample, MEL symbol 1, the codeword `0110` of context 0 (rho 1, no
/// residual), so `U_q = kappa = 1` and one MagSgn bit: the sign.
#[test]
fn a_one_sample_block_decodes_by_hand() {
    // Byte 1 is the MEL byte (top bit 0) and the VLC nibble 0x6.
    for (sign, want) in [(0x00, 384), (0x01, -384)] {
        let data = [sign, 0x62, 0x00];
        assert_eq!(
            decode(&block(1, 1, 0, &data), 8, params(0, false)).unwrap(),
            [want]
        );
    }
    // Each zero bit-plane halves it: (2 mu + 1) << (Mb - P - 1).
    let data = [0x01, 0x62, 0x00];
    assert_eq!(
        decode(&block(1, 1, 1, &data), 8, params(0, false)).unwrap(),
        [-192]
    );
    assert_eq!(
        decode(&block(1, 1, 7, &data), 8, params(0, false)).unwrap(),
        [-3]
    );
}

/// HT blocks under a maxshift of `s = 3` in a subband of `Mb = 5`, worked
/// from ISO/IEC 15444-1 Annex H and E.1 by hand. The block has `M'b = 8`
/// planes (H-3), the cleanup pass gives `N_b = S_blk + 1` MSBs of them
/// (T.814 7.6), and H.1 then sorts each sample into region or background.
/// The values are at double scale: twice the reconstruction `Rq`.
#[test]
fn an_ht_maxshift_block_decodes_by_hand() {
    let roi = params(3, false);
    // `S_blk = 0`, `mu = 1`: `N_b = 1 < Mb`, so H.1 step 2 changes nothing
    // and the one MSB is read against the `Mb = 5` planes: `q = 2^4 = 16`,
    // and E-6 with `r = 1/2` adds `2^(Mb - N_b) / 2 = 8`. `Rq = 24`.
    for (sign, want) in [(0x00, 48), (0x01, -48)] {
        let data = [sign, 0x62, 0x00];
        assert_eq!(decode(&block(1, 1, 0, &data), 5, roi).unwrap(), [want]);
    }
    // `S_blk = 7`, `mu = 1`: only MSB 8 is set, so the first `Mb` MSBs are
    // zero and the sample is background (step 4). Its MSBs move up three
    // places (H-1), `N_b = 8 - 3 = Mb` (H-2), so `q = 1` and `r = 1/2` gives
    // `Rq = -1.5`.
    let data = [0x01, 0x62, 0x00];
    assert_eq!(decode(&block(1, 1, 7, &data), 5, roi).unwrap(), [-3]);
    // `S_blk = 7`, `mu = 8`: MSB 5 is set, one of the first `Mb`, so the
    // sample is region (step 3). `N_b` becomes `Mb`, so its value is its
    // first five MSBs, `00001`: `q = 1`, with every plane known. This
    // decoder, like OpenJPEG, adds no reconstruction offset there (`r = 0`,
    // which E.1.1.2 leaves to the decoder): `Rq = 1`.
    for (data, want) in [
        (&[0xA6, 0x31, 0x47, 0x74, 0x00][..], 2),
        (&[0x47, 0x47, 0xF3, 0x00][..], -2),
    ] {
        assert_eq!(decode(&block(1, 1, 7, data), 5, roi).unwrap(), [want]);
    }
    // Without the maxshift the same blocks are refused: seven zero
    // bit-planes leave none of `Mb = 5` to code.
    assert!(matches!(
        decode(&block(1, 1, 7, &data), 5, params(0, false)),
        Err(Error::Codestream(_))
    ));
}

/// The maxshift raises the planes a refinement pass needs: a block whose
/// cleanup pass reaches `Mb`'s last plane still has `s` planes below it.
#[test]
fn passes_run_counts_the_planes_a_maxshift_adds() {
    let b = refined(1, 2, 7, &[0x00, 0x62, 0x00], &[0x01], 3);
    assert_eq!(passes_run(&b, 8, 0), 1);
    assert_eq!(passes_run(&b, 8, 1), 3);
}

#[test]
fn a_block_without_passes_or_planes_is_zero() {
    let mut b = block(3, 2, 0, &[0xAA, 0xBB]);
    b.num_passes = 0;
    assert_eq!(decode(&b, 8, params(0, false)).unwrap(), [0; 6]);
    let b = block(3, 2, 0, &[0xAA, 0xBB]);
    assert_eq!(decode(&b, 0, params(0, false)).unwrap(), [0; 6]);
}

#[test]
fn passes_run_counts_the_cleanup_pass() {
    let mut b = block(4, 4, 0, &[0, 0]);
    assert_eq!(passes_run(&b, 8, 0), 1);
    b.num_passes = 0;
    assert_eq!(passes_run(&b, 8, 0), 0);
}

// Refinement passes.

/// `block` with `passes` coding passes, the ones after the cleanup pass in a
/// refinement segment of `refinement`.
fn refined<'a>(
    width: usize,
    height: usize,
    zbp: u32,
    cleanup: &'a [u8],
    refinement: &'a [u8],
    passes: u32,
) -> CodeBlock<'a> {
    let mut b = block(width, height, zbp, cleanup);
    b.num_passes = passes;
    b.segments.push(CodedSegment {
        passes: passes - 1,
        chunks: vec![refinement],
    });
    b
}

/// The cleanup segment of a 1x2 block that leaves (0, 0) at +384 under
/// `Mb = 8`, `P = 0`, and (0, 1) insignificant.
const TOP_ONLY: [u8; 3] = [0x00, 0x62, 0x00];

#[test]
fn cleanup_and_sigprop_decode_by_hand() {
    assert_eq!(
        decode(&block(1, 2, 0, &TOP_ONLY), 8, params(0, false)).unwrap(),
        [384, 0]
    );
    // SigProp visits (0, 1) beside (0, 0): bit 1 makes it significant, the
    // next bit is its sign, and it reconstructs to 3 << (shift - 1).
    for (byte, want) in [(0x01, 192), (0x03, -192)] {
        let data = [byte];
        let b = refined(1, 2, 0, &TOP_ONLY, &data, 2);
        assert_eq!(decode(&b, 8, params(0, false)).unwrap(), [384, want]);
    }
    // A SigProp bit of 0 leaves it insignificant.
    let b = refined(1, 2, 0, &TOP_ONLY, &[0x02], 2);
    assert_eq!(decode(&b, 8, params(0, false)).unwrap(), [384, 0]);
}

#[test]
fn all_three_passes_decode_by_hand() {
    // SigProp reads byte 0 forward; MagRef reads byte 1 backward. MagRef's
    // bit for (0, 0) picks (4 mu + 3) or (4 mu + 1) at the plane below.
    for (magref, want) in [(0x01, 448), (0x00, 320)] {
        let data = [0x01, magref];
        let b = refined(1, 2, 0, &TOP_ONLY, &data, 3);
        assert_eq!(decode(&b, 8, params(0, false)).unwrap(), [want, 192]);
    }
}

#[test]
fn sigprop_propagates_through_new_samples_and_signs_each_group_after() {
    // A 4x2 block whose cleanup pass leaves only (0, 0), at +384.
    let cleanup = [0x52, 0x70, 0x63, 0x00];
    assert_eq!(
        decode(&block(4, 2, 0, &cleanup), 8, params(0, false)).unwrap(),
        [384, 0, 0, 0, 0, 0, 0, 0]
    );
    // Columns 2 and 3 have no cleanup-significant neighbor, so they are
    // visited only through samples this pass made significant. The seven
    // magnitude bits come first, column by column; then the seven signs, of
    // which only the first, for (0, 1), is set.
    let b = refined(4, 2, 0, &cleanup, &[0xFF, 0x00], 2);
    assert_eq!(
        decode(&b, 8, params(0, false)).unwrap(),
        [384, 192, 192, 192, -192, 192, 192, 192]
    );
}

#[test]
fn a_set_sigprop_stuffing_bit_is_malformed() {
    let cleanup = [0x52, 0x70, 0x63, 0x00];
    let b = refined(4, 2, 0, &cleanup, &[0xFF, 0x80], 2);
    assert!(malformed_msg(&b, 8).contains("stuffing bit"));
}

#[test]
fn an_empty_refinement_segment_decodes_the_cleanup_pass() {
    // Z_blk is 1 when the cleanup segment is the only nonempty one (B.3).
    for passes in [2, 3] {
        let b = refined(1, 2, 0, &TOP_ONLY, &[], passes);
        assert_eq!(decode(&b, 8, params(0, false)).unwrap(), [384, 0]);
        assert_eq!(passes_run(&b, 8, 0), 1);
    }
}

#[test]
fn refinement_with_no_plane_left_decodes_the_cleanup_pass() {
    // With P = Mb - 1 the cleanup pass codes the last plane; OpenJPEG then
    // decodes the cleanup pass alone, and so does this.
    let b = refined(1, 2, 7, &TOP_ONLY, &[0x01, 0x01], 3);
    assert_eq!(decode(&b, 8, params(0, false)).unwrap(), [3, 0]);
    assert_eq!(passes_run(&b, 8, 0), 1);
    let b = refined(1, 2, 6, &TOP_ONLY, &[0x01, 0x01], 3);
    assert_eq!(decode(&b, 8, params(0, false)).unwrap(), [7, 3]);
    assert_eq!(passes_run(&b, 8, 0), 3);
}

#[test]
fn passes_run_counts_the_refinement_passes() {
    for passes in [2, 3] {
        let b = refined(1, 2, 0, &TOP_ONLY, &[0x01], passes);
        assert_eq!(passes_run(&b, 8, 0), u64::from(passes));
    }
}

#[test]
fn the_vertically_causal_flag_hides_the_next_stripe() {
    // A 1x5 column whose only cleanup-significant sample, (0, 4), starts the
    // second stripe. Without the flag it makes (0, 3) a SigProp candidate.
    let pass = Refinement {
        width: 1,
        height: 5,
        causal: false,
        shift: 7,
    };
    let mut coeffs = [0, 0, 0, 0, 384];
    pass.run(
        &[0x01],
        false,
        &mut coeffs,
        &mut RefinementScratch::default(),
    )
    .unwrap();
    assert_eq!(coeffs, [0, 0, 0, 192, 384]);
    let pass = Refinement {
        causal: true,
        ..pass
    };
    let mut coeffs = [0, 0, 0, 0, 384];
    pass.run(
        &[0x01],
        false,
        &mut coeffs,
        &mut RefinementScratch::default(),
    )
    .unwrap();
    assert_eq!(coeffs, [0, 0, 0, 0, 384]);
}

#[test]
fn sigprop_reads_lsb_first_unstuffs_and_zero_fills() {
    let data = [0xFF, 0x7F];
    let mut sp = SigPropReader::new(&data);
    let bits: Vec<u32> = (0..18).map(|_| sp.bit().unwrap()).collect();
    assert_eq!(bits[..15], [1; 15]);
    assert_eq!(bits[15..], [0; 3]);
    let mut sp = SigPropReader::new(&[0x02]);
    let bits: Vec<u32> = (0..10).map(|_| sp.bit().unwrap()).collect();
    assert_eq!(bits, [0, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn magref_reads_backward_lsb_first_unstuffs_and_zero_fills() {
    // The last byte follows an implied 0xFF, so 0x7F gives seven bits; 0x80
    // then follows 0x7F and gives eight.
    let data = [0x80, 0x7F];
    let mut mr = MagRefReader::new(&data);
    let bits: Vec<u32> = (0..18).map(|_| mr.bit()).collect();
    assert_eq!(bits[..7], [1; 7]);
    assert_eq!(bits[7..15], [0, 0, 0, 0, 0, 0, 0, 1]);
    assert_eq!(bits[15..], [0; 3]);
    // A set stuffing bit is skipped (7.1.6 and its note): the last byte
    // 0xFF gives seven ones, and the next byte's bits follow. OpenJPEG ORs
    // the skipped bit into bit 7 here.
    let data = [0x00, 0xFF];
    let mut mr = MagRefReader::new(&data);
    let bits: Vec<u32> = (0..15).map(|_| mr.bit()).collect();
    assert_eq!(bits[..7], [1; 7]);
    assert_eq!(bits[7..], [0; 8]);
    // After a byte at or below 0x8F there is no stuffing bit.
    let data = [0x7F, 0x8F, 0x00];
    let mut mr = MagRefReader::new(&data);
    let bits: Vec<u32> = (0..24).map(|_| mr.bit()).collect();
    assert_eq!(bits[16..], [1, 1, 1, 1, 1, 1, 1, 0]);
}

// Refusals, one per reason.

/// A maxshift counts against the plane limit: `Mb + s` above 30 is refused
/// as the same `Limit`, and 30 decodes.
#[test]
fn a_maxshift_past_the_plane_limit_is_a_limit() {
    let data = [0x01, 0x62, 0x00];
    assert!(matches!(
        decode(&block(1, 1, 0, &data), 28, params(3, false)),
        Err(Error::Limit(m)) if m.contains("31 bit-planes") && m.contains("maxshift of 3")
    ));
    assert!(decode(&block(1, 1, 0, &data), 27, params(3, false)).is_ok());
}

/// A cleanup magnitude at the bound `CAP` signals is refused (T.814 A.3.7,
/// 8.7.3: every `mu` is below it); one below it decodes.
#[test]
fn a_cleanup_magnitude_at_the_cap_bound_is_malformed() {
    // `mu = 3`, two bits: below 2^2, not below 2^1.
    let data = [0x00, 0x27, 0xF3, 0x00];
    let bound = |bits| Params {
        bound_bits: Some(bits),
        ..params(0, false)
    };
    assert_eq!(decode(&block(1, 1, 1, &data), 8, bound(2)).unwrap(), [448]);
    match decode(&block(1, 1, 1, &data), 8, bound(1)) {
        Err(Error::Codestream(m)) => {
            assert!(m.contains("magnitude 3 is not below 2^1"), "{m}");
        }
        other => panic!("expected a Codestream error, got {other:?}"),
    }
    // A bound of 64 bits or more holds of every magnitude.
    assert!(decode(&block(1, 1, 1, &data), 8, bound(74)).is_ok());
}

#[test]
fn more_than_one_ht_set_is_malformed() {
    let data = [0x00, 0x62, 0x00];
    let mut b = refined(1, 2, 0, &data, &[0x01], 4);
    assert!(malformed_msg(&b, 8).contains("4 coding passes, more than the 3"));
    b.num_passes = 3;
    assert!(decode(&b, 8, params(0, false)).is_ok());
}

#[test]
fn too_many_bit_planes_is_a_limit() {
    let data = [0x01, 0x62, 0x00];
    assert!(matches!(
        decode(&block(1, 1, 0, &data), 31, params(0, false)),
        Err(Error::Limit(m)) if m.contains("31 bit-planes")
    ));
    assert!(decode(&block(1, 1, 0, &data), 30, params(0, false)).is_ok());
}

#[test]
fn zero_bit_planes_must_leave_a_plane() {
    let data = [0x01, 0x62, 0x00];
    let msg = malformed_msg(&block(1, 1, 8, &data), 8);
    assert!(msg.contains("leaving none to code"), "{msg}");
}

#[test]
fn a_segment_under_two_bytes_is_malformed() {
    let msg = malformed_msg(&block(1, 1, 0, &[0x02]), 8);
    assert!(msg.contains("1 bytes, outside 2..65535"), "{msg}");
}

/// `Lcup` is under 65535 (7.1.1). OpenJPEG 2.5.0 decodes a 65535-byte
/// segment.
#[test]
fn a_segment_of_65535_bytes_is_malformed() {
    // MagSgn byte 0x01 (negative), then the one-sample MEL and VLC bytes.
    let mut data = vec![0x01];
    data.resize(65532, 0);
    data.extend([0x62, 0x00]);
    assert_eq!(
        decode(&block(1, 1, 0, &data), 8, params(0, false)).unwrap(),
        [-384]
    );
    data.insert(1, 0);
    let msg = malformed_msg(&block(1, 1, 0, &data), 8);
    assert!(msg.contains("65535 bytes, outside 2..65535"), "{msg}");
}

#[test]
fn scup_outside_its_range_is_malformed() {
    // Scup = 1, under 2.
    let msg = malformed_msg(&block(1, 1, 0, &[0x00, 0x01, 0x00]), 8);
    assert!(msg.contains("Scup = 1"), "{msg}");
    // Scup = 4, over Lcup = 3.
    let msg = malformed_msg(&block(1, 1, 0, &[0x00, 0x04, 0x00]), 8);
    assert!(msg.contains("Scup = 4"), "{msg}");
    // Scup = Lcup is allowed: no MagSgn bytes at all.
    assert!(decode(&block(1, 1, 0, &[0x62, 0x00]), 8, params(0, false)).is_ok());
    // Scup = 4079 is the largest allowed, and 4080 is refused even in a
    // segment long enough to hold it, as OpenJPEG 2.5.0 does.
    let mut data = vec![0; 4081];
    data[4079..].copy_from_slice(&[0x6F, 254]);
    assert_eq!(
        decode(&block(1, 1, 0, &data), 8, params(0, false)).unwrap(),
        [384]
    );
    data[4079..].copy_from_slice(&[0x60, 255]);
    let msg = malformed_msg(&block(1, 1, 0, &data), 8);
    assert!(msg.contains("Scup = 4080"), "{msg}");
}

// The vectors below were found by a seeded random search over short
// segments; each is the first that fails for its reason.

#[test]
fn significance_outside_the_block_is_malformed() {
    let msg = malformed_msg(&block(3, 3, 1, &[0x59, 0xCC, 0xE4, 0x00]), 8);
    assert!(msg.contains("outside the block"), "{msg}");
}

#[test]
fn a_vlc_stream_reaching_magsgn_is_malformed() {
    let msg = malformed_msg(&block(3, 1, 1, &[0xD6, 0xB3, 0x72, 0x00]), 8);
    assert!(msg.contains("VLC stream runs into"), "{msg}");
}

#[test]
fn a_set_magsgn_stuffing_bit_is_malformed() {
    let data = [0xFF, 0xDF, 0x39, 0x6F, 0xD4, 0xA5, 0x00];
    let msg = malformed_msg(&block(4, 2, 1, &data), 8);
    assert!(msg.contains("stuffing bit set"), "{msg}");
}

#[test]
fn a_magsgn_stream_past_its_end_is_malformed() {
    let msg = malformed_msg(&block(4, 4, 2, &[0x6D, 0x49, 0x64, 0x00]), 30);
    assert!(msg.contains("MagSgn stream runs past"), "{msg}");
}

#[test]
fn an_exponent_bound_over_the_coded_planes_is_malformed() {
    let data = [0x0B, 0x85, 0x54, 0x91, 0xE1, 0x56, 0x00];
    let msg = malformed_msg(&block(4, 2, 2, &data), 8);
    assert!(msg.contains("U_q = 21 exceeds"), "{msg}");
}

/// A conforming block whose `U_q` reaches the bound `P + 2`: `mu = 3` has
/// the `P + 1 = 2` bits the block codes, and `2 mu - 1 = 5` has 3. The
/// coefficient is `7 << (Mb - P - 1)` at double scale, so the image sample
/// is 224, as OpenJPEG 2.5.0 decodes the same block.
#[test]
fn an_exponent_bound_of_p_plus_2_decodes() {
    for (sign, want) in [(0x00, 448), (0x01, -448)] {
        let data = [sign, 0x27, 0xF3, 0x00];
        assert_eq!(
            decode(&block(1, 1, 1, &data), 8, params(0, false)).unwrap(),
            [want]
        );
    }
}

/// `U_q` may reach `P + 2` (7.6) and no further.
#[test]
fn an_exponent_bound_one_over_the_allowance_is_malformed() {
    let msg = malformed_msg(&block(4, 2, 1, &[0xB5, 0x0B, 0xB2, 0xB4, 0x00]), 30);
    assert!(
        msg.contains("U_q = 4 exceeds zero bit-planes + 2 = 3"),
        "{msg}"
    );
}

/// A residual whose suffix is 28 or more adds four times its extension: the
/// bound here is only reachable with the extension.
#[test]
fn a_residual_extension_counts_four_times() {
    let data = [0x15, 0x2C, 0x7E, 0x07, 0xF5, 0x00];
    let msg = malformed_msg(&block(1, 1, 11, &data), 30);
    assert!(msg.contains("U_q = 62 exceeds"), "{msg}");
}

/// Only a magnitude of `P + 2` bits, which the block cannot code, overflows
/// 32 bits. OpenJPEG 2.5.0 wraps it.
#[test]
fn a_magnitude_past_the_coded_planes_is_malformed() {
    let msg = malformed_msg(&block(4, 2, 0, &[0x0F, 0x32, 0xC4, 0x00]), 30);
    assert!(msg.contains("magnitude past its bit-planes"), "{msg}");
}

/// Random segments, geometries and bit-plane counts never panic, and a
/// successful decode fills exactly the block.
#[test]
fn random_segments_never_panic() {
    let mut rng = Lcg(0x5EED);
    let mut decoded = 0;
    for _ in 0..20_000 {
        let len = 2 + (rng.next() % 24) as usize;
        let mut bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        // Usually a valid Scup, so the decoder gets past its first checks.
        if !rng.next().is_multiple_of(8) {
            let scup = 2 + rng.next() as usize % (len - 1);
            bytes[len - 1] = (scup >> 4) as u8;
            bytes[len - 2] = (bytes[len - 2] & 0xF0) | (scup & 0xF) as u8;
        }
        let w = 1 + (rng.next() % 16) as usize;
        let h = 1 + (rng.next() % 16) as usize;
        let mb = (rng.next() % 32) as u32;
        let zbp = (rng.next() % 32) as u32;
        let refinement: Vec<u8> = (0..rng.next() % 8).map(|_| rng.next() as u8).collect();
        let passes = 1 + (rng.next() % 3) as u32;
        let causal = rng.next().is_multiple_of(2);
        let b = refined(w, h, zbp, &bytes, &refinement, passes);
        // Mostly without a maxshift or a bound, so enough blocks decode.
        let roi_shift = if rng.next().is_multiple_of(4) {
            (rng.next() % 40) as u8
        } else {
            0
        };
        let bound_bits = rng
            .next()
            .is_multiple_of(4)
            .then(|| (rng.next() % 80) as u32);
        let p = Params {
            roi_shift,
            causal,
            bound_bits,
        };
        if let Ok(c) = decode(&b, mb, p) {
            assert_eq!(c.len(), w * h);
            decoded += 1;
        }
    }
    assert!(decoded > 100, "only {decoded} random blocks decoded");
}
