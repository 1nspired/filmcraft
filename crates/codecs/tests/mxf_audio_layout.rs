//! The MXF source must bound header-derived channels before accumulation or allocation.
//! In-memory original PCM fixtures run without ffmpeg, model weights or external media.
use std::{io::Cursor, panic::catch_unwind, sync::Arc};

use filmcraft_codecs::MxfSource;
use filmcraft_media::{MediaKind, MediaSource};
use filmcraft_mxf::{MxfWriter, PackageIds, Pattern, Rational, SoundDesc, WriterConfig};

const FRAMES: usize = 1920;

fn sample(channel: usize, frame: usize) -> i16 {
    ((channel * 97 + frame * 11) % 32000) as i16 - 16000
}

fn pcm_mxf(channels: &[u32]) -> Vec<u8> {
    let mut config = WriterConfig::new(Pattern::Op1a, Rational::new(25, 1), PackageIds::from_seed("channel-bounds", "Generated PCM"));
    config.sound = channels.iter().map(|&channels| SoundDesc { sample_rate: 48_000, channels, bits: 16 }).collect();
    let mut writer = MxfWriter::new(Cursor::new(Vec::new()), config).unwrap();
    let mut first_channel = 0;
    for (track, &channels) in channels.iter().enumerate() {
        let mut pcm = Vec::new();
        for frame in 0..FRAMES {
            for channel in 0..channels as usize {
                pcm.extend_from_slice(&sample(first_channel + channel, frame).to_le_bytes());
            }
        }
        writer.push_sound(track, &pcm).unwrap();
        first_channel += channels as usize;
    }
    writer.finish().unwrap().into_inner()
}

fn mutate_channel_counts(mut bytes: Vec<u8>, channels: u32) -> Arc<[u8]> {
    let offsets: Vec<_> = bytes.windows(8).enumerate().filter(|(_, w)| w[..4] == [0x3d, 0x07, 0, 4]).map(|(i, _)| i + 4).collect();
    assert!(offsets.len() >= 2, "the generated descriptors must contain ChannelCount");
    for offset in offsets {
        bytes[offset..offset + 4].copy_from_slice(&channels.to_be_bytes());
    }
    let parsed = filmcraft_mxf::open(&bytes).unwrap();
    assert_eq!(parsed.tracks.iter().filter_map(|track| track.sound.as_ref()).count(), 2);
    assert!(parsed.tracks.iter().filter_map(|track| track.sound.as_ref()).all(|sound| sound.channels == channels));
    bytes.into()
}

fn assert_channel_error(bytes: Arc<[u8]>) {
    let result = catch_unwind(|| MxfSource::open("generated-channel-count.mxf", bytes));
    assert!(result.is_ok(), "malformed channel metadata must return an error without panicking");
    match result.unwrap() {
        Err(error) => assert!(error.to_string().contains("channel"), "{error}"),
        Ok(_) => panic!("unsupported channel metadata was accepted"),
    }
}

#[test]
fn invalid_sound_channel_counts_return_errors_without_panicking() {
    let valid = pcm_mxf(&[1, 1]);
    for channels in [0x8000_0000, u32::MAX, 257, 0] {
        assert_channel_error(mutate_channel_counts(valid.clone(), channels));
    }
}

#[test]
fn combined_sound_channels_above_limit_are_refused_without_panicking() {
    for tracks in [[128, 129], [129, 129], [256, 1]] {
        assert_channel_error(pcm_mxf(&tracks).into());
    }
}

fn assert_sample_exact(channels: &[u32]) {
    let source = MxfSource::open("generated-multitrack.mxf", pcm_mxf(channels).into()).unwrap();
    let total = channels.iter().sum::<u32>() as usize;
    assert_eq!(source.info().kind, MediaKind::AudioOnly);
    assert_eq!(source.info().audio_streams.len(), 1);
    assert_eq!(source.info().audio().unwrap().channels as usize, total);
    let layout = source.audio_channel_layout().unwrap().unwrap();
    assert_eq!(layout.kind, "mxf-pcm-v1");
    assert_eq!(layout.sample_rate, 48_000);
    assert_eq!(layout.channels as usize, total);
    assert_eq!(layout.sample_frames, FRAMES.to_string());
    assert_eq!(layout.tracks.iter().map(|track| track.channels).collect::<Vec<_>>(), channels);
    let mut first = 0;
    for track in &layout.tracks {
        assert_eq!(track.first_channel, first);
        assert_eq!(track.first_sample, "0");
        assert_eq!(track.first_chunk_bytes, (FRAMES as u32 * track.channels * 2).to_string());
        assert_eq!(track.format, "pcm");
        assert_eq!(track.wrapping, "frame");
        first += track.channels;
    }
    for (start, count) in [(0, 16), (113, 29), (FRAMES - 2, 2)] {
        let pcm = source.audio(start as i64, count, 48_000).unwrap();
        assert_eq!(pcm.channels.len(), total);
        for (channel, plane) in pcm.channels.iter().enumerate() {
            assert_eq!(plane.len(), count);
            for (offset, &value) in plane.iter().enumerate() {
                assert_eq!(value, f32::from(sample(channel, start + offset)) / 32768.0, "channel {channel} sample {}", start + offset);
            }
        }
    }
}

#[test]
fn combined_channel_limit_preserves_every_track_and_channel() {
    assert_sample_exact(&[128, 128]);
}

#[test]
fn mono_and_interleaved_pcm_remain_sample_exact() {
    for channels in [&[4][..], &[1, 1, 1, 1, 1, 1, 1, 1], &[2, 1, 3]] {
        assert_sample_exact(channels);
    }
}
