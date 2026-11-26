//! A synthetic MXF must use the registered decoder, not force H.264 software.
//! No OS decoder or external media is needed: the recording factory owns only
//! this test's distinctive parameter sets. The old MXF path never reaches it.
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use filmcraft_codecs::{DecodedFrame, MxfSource, VideoDecoder, register_video_decoder};
use filmcraft_frame::VideoFrame;
use filmcraft_isobmff::{CodecConfig, SampleEntry};
use filmcraft_media::{FrameRequest, MediaSource};
use filmcraft_mxf::{FrameInfo, MxfWriter, PackageIds, Pattern, PictureCoding, PictureDesc, Rational, WriterConfig};
use filmcraft_time::Tick;

const SPS: &[u8] = &[0x67, 0xf4, 0x7e, 0x2a, 0x12, 0x34];
const PPS: &[u8] = &[0x68, 0x55, 0x66];
static CREATED: AtomicUsize = AtomicUsize::new(0);
static DECODED: AtomicUsize = AtomicUsize::new(0);

struct RecordingDecoder;

impl VideoDecoder for RecordingDecoder {
    fn decode(&mut self, sample: &[u8], pts: i64) -> filmcraft_codecs::Result<Vec<DecodedFrame>> {
        let nals = filmcraft_bitstream::length_prefixed_nals(sample, 4).unwrap();
        assert_eq!(nals, vec![SPS, PPS, &[0x65, 0x88, 0x99][..]]);
        DECODED.fetch_add(1, Ordering::SeqCst);
        Ok(vec![DecodedFrame { pts, frame: VideoFrame::rgba8(16, 16, vec![127; 16 * 16 * 4]), draft: false }])
    }
    fn flush(&mut self) -> Vec<DecodedFrame> {
        Vec::new()
    }
    fn reset(&mut self) {}
    fn name(&self) -> &str {
        "recording MXF hardware"
    }
}

fn recording_factory(entry: &SampleEntry) -> Option<filmcraft_codecs::Result<Box<dyn VideoDecoder>>> {
    let CodecConfig::Avc(config) = &entry.codec else { return None };
    if config.sps != vec![SPS.to_vec()] {
        return None;
    }
    assert_eq!(config.pps, vec![PPS.to_vec()]);
    assert_eq!(config.length_size, 4);
    CREATED.fetch_add(1, Ordering::SeqCst);
    Some(Ok(Box::new(RecordingDecoder)))
}

#[test]
fn mxf_avc_uses_registered_decoder_with_length_prefixed_samples() {
    register_video_decoder(recording_factory);
    let mut config = WriterConfig::new(Pattern::Op1a, Rational::new(25, 1), PackageIds::from_seed("mxf registry test", "Synthetic AVC"));
    config.picture = Some(PictureDesc::new(PictureCoding::Avc { profile_idc: 244, intra: true }, 16, 16));
    let mut writer = MxfWriter::new(Cursor::new(Vec::new()), config).unwrap();
    let mut sample = Vec::new();
    for nal in [SPS, PPS, &[0x65, 0x88, 0x99][..]] {
        sample.extend_from_slice(&[0, 0, 0, 1]);
        sample.extend_from_slice(nal);
    }
    writer.push_picture(&sample, FrameInfo::intra()).unwrap();
    let bytes: Arc<[u8]> = writer.finish().unwrap().into_inner().into();
    let source = MxfSource::open("registry.mxf", bytes).unwrap();
    let decoded = source.video_frame(FrameRequest::full(Tick::ZERO));
    assert!(CREATED.load(Ordering::SeqCst) > 0, "MXF bypassed the registered decoder: {:?}", decoded.err());
    assert!(DECODED.load(Ordering::SeqCst) > 0);
    let frame = decoded.unwrap();
    assert_eq!((frame.width, frame.height), (16, 16));
}
