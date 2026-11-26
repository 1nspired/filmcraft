//! H.264 Annex B access units through the existing sample-entry decoder registry.
//! Only framing changes (ITU-T H.264 Annex B; ISO/IEC 14496-15 avcC). NAL payloads,
//! presentation timestamps, in-band parameter sets and decoder fallback are kept.
use filmcraft_isobmff::{AvcConfig, SampleEntry};

use crate::{CodecError, DecodedFrame, Result, VideoDecoder, make_video_decoder};

pub(crate) const MAX_ACCESS_UNIT_BYTES: usize = 64 << 20;
const MAX_NALS: usize = 4096;

fn invalid(message: &str) -> CodecError {
    CodecError::Decode(format!("Annex B AVC: {message}"))
}

/// Bounded splitting: the general Annex B splitter collects every start code
/// before returning, so pathological input could allocate a much larger table.
fn nals(sample: &[u8]) -> Result<Vec<&[u8]>> {
    if sample.len() > MAX_ACCESS_UNIT_BYTES {
        return Err(invalid("access unit exceeds 64 MiB"));
    }
    let mut starts = sample.windows(3).enumerate().filter(|(_, w)| *w == [0, 0, 1]);
    let (first, _) = starts.next().ok_or_else(|| invalid("missing start code"))?;
    if sample.get(..first).is_none_or(|p| p.iter().any(|&b| b != 0)) {
        return Err(invalid("nonzero bytes before the first start code"));
    }
    let mut begin = first + 3; // first is a three-byte window inside a <=64 MiB sample
    let mut units = Vec::new();
    for end in starts.map(|(at, _)| at).chain(std::iter::once(sample.len())) {
        if units.len() >= MAX_NALS {
            return Err(invalid("too many NAL units"));
        }
        let bytes = sample.get(begin..end).ok_or_else(|| invalid("overlapping start codes"))?;
        let length = bytes.iter().rposition(|&b| b != 0).map_or(0, |at| at + 1);
        let unit = bytes.get(..length).filter(|u| !u.is_empty()).ok_or_else(|| invalid("empty NAL unit"))?;
        if unit.first().is_none_or(|h| h & 0x80 != 0 || h & 0x1f == 0) {
            return Err(invalid("invalid NAL header"));
        }
        units.push(unit);
        begin = end + 3; // unused after the final sentinel; bounded by the sample cap
    }
    Ok(units)
}

fn length_prefixed(sample: &[u8]) -> Result<Vec<u8>> {
    let units = nals(sample)?;
    let capacity = units.iter().try_fold(0usize, |n, u| n.checked_add(4)?.checked_add(u.len())).ok_or_else(|| invalid("sample size overflow"))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(capacity).map_err(|_| invalid("cannot allocate framed access unit"))?;
    for unit in units {
        let length = u32::try_from(unit.len()).map_err(|_| invalid("NAL unit too large"))?;
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(unit);
    }
    Ok(bytes)
}

/// Missing out-of-band parameter sets retain the old software Annex B path:
/// streams may deliver them later. Complete sets use the registered factories,
/// including the platform's existing hardware/setting/fallback policy.
pub(crate) fn decoder(first_sample: &[u8], width: u16, height: u16) -> Result<Box<dyn VideoDecoder>> {
    let mut sps = Vec::new();
    let mut pps = Vec::new();
    for unit in nals(first_sample)? {
        let target = match unit.first().map(|h| h & 0x1f) {
            Some(7) => &mut sps,
            Some(8) => &mut pps,
            _ => continue,
        };
        // avcC stores each parameter-set length in a u16. Refuse instead of
        // allowing AvcConfig::to_bytes to truncate the length on conversion.
        if unit.len() > u16::MAX as usize {
            return Err(invalid("parameter set exceeds avcC length limit"));
        }
        if !target.iter().any(|p: &Vec<u8>| p.as_slice() == unit) {
            target.push(unit.to_vec());
        }
    }
    if sps.len() > 31 || pps.len() > 255 {
        return Err(invalid("too many parameter sets for avcC"));
    }
    if sps.is_empty() || pps.is_empty() {
        return Ok(Box::new(crate::video::H264Decoder::annexb()));
    }
    let entry = SampleEntry::avc(AvcConfig::new(sps, pps, 4), width, height);
    let inner = make_video_decoder(&entry)?;
    Ok(Box::new(AnnexBDecoder { inner }))
}

struct AnnexBDecoder {
    inner: Box<dyn VideoDecoder>,
}

impl VideoDecoder for AnnexBDecoder {
    fn decode(&mut self, sample: &[u8], pts: i64) -> Result<Vec<DecodedFrame>> {
        self.inner.decode(&length_prefixed(sample)?, pts)
    }
    fn flush(&mut self) -> Vec<DecodedFrame> {
        self.inner.flush()
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn intra_only(&self) -> bool {
        self.inner.intra_only()
    }
    fn is_random_access(&self, sample: &[u8]) -> Option<bool> {
        nals(sample).ok().map(|units| units.iter().any(|n| n.first().is_some_and(|h| h & 0x1f == 5)))
    }
    fn is_disposable(&self, sample: &[u8]) -> bool {
        length_prefixed(sample).is_ok_and(|bytes| self.inner.is_disposable(&bytes))
    }
    fn set_draft(&mut self, on: bool) {
        self.inner.set_draft(on);
    }
    fn thread_limited(&self) -> bool {
        self.inner.thread_limited()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_conversion_preserves_headers_payloads_and_emulation_prevention() {
        let sample = [0, 0, 0, 1, 0x67, 0x64, 0, 0, 3, 1, 0, 0, 1, 0x68, 0x80, 0, 0, 0, 1, 0x65, 0x88, 0, 0];
        let converted = length_prefixed(&sample).unwrap();
        let actual = filmcraft_bitstream::length_prefixed_nals(&converted, 4).unwrap();
        assert_eq!(actual, vec![&[0x67, 0x64, 0, 0, 3, 1][..], &[0x68, 0x80][..], &[0x65, 0x88][..]]);
    }

    #[test]
    fn malformed_and_pathological_packets_are_errors_not_panics() {
        for sample in [&[][..], &[0, 0, 1], &[9, 0, 0, 1, 0x65], &[0, 0, 1, 0x80], &[0, 0, 1, 0], &[0, 0, 1, 0, 0, 1, 0x65]] {
            assert!(std::panic::catch_unwind(|| length_prefixed(sample)).unwrap().is_err(), "{sample:?}");
        }
        let many = [0, 0, 1, 0x65].repeat(MAX_NALS + 1);
        assert!(length_prefixed(&many).is_err());
        let long_set = [vec![0, 0, 1, 0x67], vec![0x55; u16::MAX as usize], vec![0, 0, 1, 0x68, 0x80]].concat();
        assert!(decoder(&long_set, 16, 16).is_err());
    }

    #[test]
    fn truncation_and_seeded_mutation_never_panic() {
        let sample = [0, 0, 0, 1, 0x67, 0x64, 0x80, 0, 0, 1, 0x68, 0x80, 0, 0, 1, 0x65, 0x88];
        for end in 0..=sample.len() {
            assert!(std::panic::catch_unwind(|| length_prefixed(&sample[..end])).is_ok());
        }
        for index in 0..sample.len() {
            for bit in 0..8 {
                let mut changed = sample;
                changed[index] ^= 1 << bit;
                assert!(std::panic::catch_unwind(|| length_prefixed(&changed)).is_ok());
            }
        }
    }

    #[test]
    fn incomplete_parameter_sets_keep_the_software_annex_b_decoder() {
        let decoder = decoder(&[0, 0, 1, 0x65, 0x88], 16, 16).unwrap();
        assert_eq!(decoder.name(), "FilmCraft H.264");
    }
}
