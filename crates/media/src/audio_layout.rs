//! Read-only channel correspondence reported by a freshly opened source.
//! Byte/sample positions are decimal strings for lossless JSON transport.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceAudioLayout {
    pub kind: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub sample_frames: String,
    /// Groups appear in the actual output channel order of native stream zero.
    pub tracks: Vec<SourceAudioTrack>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceAudioTrack {
    pub track_id: u32,
    pub track_number: String,
    pub first_channel: u32,
    pub channels: u32,
    pub bits: u32,
    pub format: String,
    pub wrapping: String,
    pub first_sample: String,
    pub first_chunk_offset: String,
    pub first_chunk_bytes: String,
}
