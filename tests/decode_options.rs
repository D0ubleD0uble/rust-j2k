//! `DecodeOptions` compares by value, and every public way a limit enters it
//! stores `Raised(1)` as `Default`, which sets the same bound. The fields are
//! private, so the builders are the only way in.

use rust_j2k::{ColorOutput, DecodeOptions, Region, SampleLimit, WorkLimit};

/// `Raised(1)` through each limit builder equals the default options, and
/// the getters report `Default`.
#[test]
fn raised_one_through_a_builder_equals_the_default() {
    let default = DecodeOptions::default();

    let packet = DecodeOptions::default().with_packet_work_limit(WorkLimit::Raised(1));
    assert_eq!(packet, default);
    assert_eq!(packet.packet_work_limit(), &WorkLimit::Default);

    let block = DecodeOptions::default().with_code_block_work_limit(WorkLimit::Raised(1));
    assert_eq!(block, default);
    assert_eq!(block.code_block_work_limit(), &WorkLimit::Default);

    let samples = DecodeOptions::default().with_sample_limit(SampleLimit::Raised(1));
    assert_eq!(samples, default);
    assert_eq!(samples.sample_limit(), &SampleLimit::Default);

    // Chained, and on top of a limit already raised.
    let all = DecodeOptions::default()
        .with_packet_work_limit(WorkLimit::Raised(3))
        .with_packet_work_limit(WorkLimit::Raised(1))
        .with_code_block_work_limit(WorkLimit::Raised(1))
        .with_sample_limit(SampleLimit::Raised(1));
    assert_eq!(all, default);
}

/// Equality sees every option, so an option that differs is a different
/// value.
#[test]
fn options_that_differ_are_not_equal() {
    let default = DecodeOptions::default();
    let differing = [
        DecodeOptions::default().with_resolution_reduction(1),
        DecodeOptions::default().with_max_quality_layers(Some(1)),
        DecodeOptions::default().with_region(Region::Tile(0)),
        DecodeOptions::default().with_components(Some(vec![0])),
        DecodeOptions::default().with_raw_channels(true),
        DecodeOptions::default().with_color(ColorOutput::Srgb),
        DecodeOptions::default().with_packet_work_limit(WorkLimit::Raised(2)),
        DecodeOptions::default().with_code_block_work_limit(WorkLimit::Unlimited),
        DecodeOptions::default().with_sample_limit(SampleLimit::AtMost(8)),
        DecodeOptions::default().with_strict(false),
    ];
    for (i, options) in differing.iter().enumerate() {
        assert_ne!(options, &default, "option {i}");
        assert_eq!(options, &options.clone(), "option {i}");
    }
}
