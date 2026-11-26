//! Native MXF AVC High 4:2:2 10-bit decode, through the registered factory.
//! ffmpeg is an external fixture generator / pixel oracle, never linked.
#![cfg(target_os = "macos")]

mod common;

use filmcraft_codecs::MxfSource;
use filmcraft_frame::{Chroma, PixelData};
use filmcraft_media::{FrameRequest, MediaSource};
use filmcraft_time::FrameRate;

static HARDWARE_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn mxf_avc_422_10bit_matches_ffmpeg_and_seeks_when_hardware_is_available() {
    let _guard = HARDWARE_TEST.lock().unwrap();
    let ff = filmcraft_testkit::require_ffmpeg!();
    let args = [
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x240:rate=25",
        "-t",
        "0.48",
        "-c:v",
        "libx264",
        "-profile:v",
        "high422",
        "-pix_fmt",
        "yuv422p10le",
        "-g",
        "6",
        "-bf",
        "0",
        "-an",
    ];
    let path = common::fixture(&ff, "mxf_avc_422_10_registry.mxf", &args).expect("generate MXF AVC fixture");
    filmcraft_platform::register();
    let bytes: std::sync::Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let source = MxfSource::open("10-bit MXF", bytes.clone()).unwrap();
    let before = filmcraft_codecs::hw::hw_stats();
    let first = source.video_frame(FrameRequest::full(FrameRate::FPS_25.tick_of(0)));
    if let Err(error) = &first {
        if std::env::var_os("FILMCRAFT_REQUIRE_MXF_HARDWARE").is_some() {
            panic!("required MXF hardware decode failed: {error}");
        }
        eprintln!("SKIPPED: no working MXF AVC 4:2:2 10-bit hardware path: {error}");
        return;
    }
    first.unwrap();
    assert!(filmcraft_codecs::hw::hw_stats().frames > before.frames, "must actually decode using hardware");
    let decoded = std::process::Command::new(&ff)
        .args(["-v", "error", "-i", path.to_str().unwrap(), "-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "yuv422p10le", "-"])
        .output()
        .unwrap();
    assert!(decoded.status.success(), "{}", String::from_utf8_lossy(&decoded.stderr));
    let oracle: Vec<u16> = decoded.stdout.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
    let (y, c) = (320 * 240, 160 * 240);
    let frame_size = y + 2 * c;
    assert_eq!(oracle.len(), 12 * frame_size);
    let compare = |source: &MxfSource, index: usize| {
        let frame = source.video_frame(FrameRequest::full(FrameRate::FPS_25.tick_of(index as i64))).unwrap();
        assert_eq!((frame.width, frame.height), (320, 240));
        let PixelData::Yuv16 { planes, chroma, bits, .. } = &frame.data else { panic!("10-bit planar output required") };
        assert_eq!((*chroma, *bits), (Chroma::C422, 10));
        let raw = &oracle[index * frame_size..(index + 1) * frame_size];
        assert_eq!(planes[0].as_slice(), &raw[..y], "luma of frame {index}");
        assert_eq!(planes[1].as_slice(), &raw[y..y + c], "Cb of frame {index}");
        assert_eq!(planes[2].as_slice(), &raw[y + c..], "Cr of frame {index}");
    };
    for index in 0..12 {
        compare(&source, index);
    }
    // A fresh source for each seek rules out serving a previously cached frame.
    for index in [11, 3, 0, 7] {
        compare(&MxfSource::open("seek MXF", bytes.clone()).unwrap(), index);
    }
    filmcraft_codecs::hw::set_hardware_decoding(false);
    let off_before = filmcraft_codecs::hw::hw_stats().frames;
    let off = MxfSource::open("hardware off", bytes).unwrap().video_frame(FrameRequest::full(FrameRate::FPS_25.tick_of(0)));
    filmcraft_codecs::hw::set_hardware_decoding(true);
    assert!(off.is_err(), "unsupported software 4:2:2 must not pretend to decode");
    assert_eq!(filmcraft_codecs::hw::hw_stats().frames, off_before);
}

/// Explicit local-only camera check. No hard-coded private path, no fixture
/// upload/copy, no whole-file read: even spanned multi-gigabyte clips stream.
#[test]
fn real_mxf_sample_matches_ffmpeg_when_requested() {
    let Some(path) = std::env::var_os("FILMCRAFT_MXF_SAMPLE").map(std::path::PathBuf::from) else { return };
    let _guard = HARDWARE_TEST.lock().unwrap();
    let ff = filmcraft_testkit::require_ffmpeg!();
    filmcraft_platform::register();
    let reader = filmcraft_media::reader::FileReader::open(&path).unwrap();
    let source = MxfSource::open_reader("opt-in camera sample", std::sync::Arc::new(reader)).unwrap();
    let video = source.info().video.clone().unwrap();
    let before = filmcraft_codecs::hw::hw_stats().frames;
    let reference = std::process::Command::new(ff)
        .args(["-v", "error", "-i"])
        .arg(&path)
        .args(["-map", "0:v:0", "-frames:v", "3", "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "yuv422p10le", "-"])
        .output()
        .unwrap();
    assert!(reference.status.success(), "{}", String::from_utf8_lossy(&reference.stderr));
    let oracle: Vec<u16> = reference.stdout.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
    let y = video.width as usize * video.height as usize;
    let c = (video.width as usize).div_ceil(2) * video.height as usize;
    let size = y + 2 * c;
    assert_eq!(oracle.len(), 3 * size);
    for index in 0..3 {
        let frame = source.video_frame(FrameRequest::full(video.frame_rate.tick_of(index))).unwrap();
        let PixelData::Yuv16 { planes, chroma, bits, .. } = &frame.data else { panic!("10-bit planar output required") };
        assert_eq!((*chroma, *bits), (Chroma::C422, 10));
        let raw = &oracle[index as usize * size..(index as usize + 1) * size];
        for (plane, expected) in planes.iter().zip([&raw[..y], &raw[y..y + c], &raw[y + c..]]) {
            assert_eq!(plane.len(), expected.len());
            let mismatch = plane.iter().zip(expected).enumerate().find(|(_, (actual, reference))| actual != reference);
            assert!(mismatch.is_none(), "camera frame {index}: first differing sample {mismatch:?}");
        }
    }
    assert!(filmcraft_codecs::hw::hw_stats().frames > before, "camera decode must actually use hardware");
}
