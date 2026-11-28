//! Optional clip-local envelope. No lane is cleared/remixed, so simultaneous
//! edge fades multiply and leave unrelated material untouched.
use filmcraft_project::TrackItem;
use filmcraft_time::Tick;
#[derive(Default)]
pub(crate) struct Envelope {
    input: i64,
    output: i64,
    out_start: i64,
    in_linear: bool,
    out_linear: bool,
}
// ECMAScript toFixed(3), for finite nonnegative bounded timeline seconds:
// round the actual binary64 rational, not an already-rounded x * 1000.
fn milliseconds(value: f64) -> i64 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    let bits = value.to_bits();
    let power = ((bits >> 52) & 0x7ff) as i32 - 1023 - 52;
    let numerator = (((bits & ((1u64 << 52) - 1)) | (1u64 << 52)) as u128) * 1000;
    if power >= 0 {
        return numerator.checked_shl(power as u32).unwrap_or(i64::MAX as u128).min(i64::MAX as u128) as i64;
    }
    let shift = (-power) as u32;
    if shift >= 128 {
        return 0;
    }
    let denominator = 1u128 << shift;
    ((numerator / denominator) + u128::from(numerator % denominator >= denominator / 2)).min(i64::MAX as u128) as i64
}
fn samples(microseconds: i64, rate: u32) -> i64 {
    ((microseconds as i128 * rate as i128 + 500_000) / 1_000_000).min(i64::MAX as i128) as i64
}
impl Envelope {
    pub(crate) fn for_item(item: &TrackItem, rate: u32) -> Self {
        let Some(effect) = item.effect("clip_audio_fades").filter(|e| e.enabled) else {
            return Self::default();
        };
        let duration = |key| effect.f64_at(key, Tick::ZERO).clamp(0.0, 5.0);
        let input = duration("in_seconds");
        let output = duration("out_seconds");
        Self {
            input: samples((input * 1_000_000.0).round() as i64, rate),
            output: samples((output * 1_000_000.0).round() as i64, rate),
            out_start: samples(milliseconds((item.duration.seconds() - output).max(0.0)) * 1000, rate),
            in_linear: effect.param("in_linear").and_then(|p| p.value.as_bool()).unwrap_or(false),
            out_linear: effect.param("out_linear").and_then(|p| p.value.as_bool()).unwrap_or(false),
        }
    }
    pub(crate) fn gain(&self, position: i64) -> f32 {
        let shape = |progress: f64, linear: bool| if linear { progress } else { (progress * std::f64::consts::FRAC_PI_2).sin() };
        let input = if self.input > 0 { shape((position as f64 / self.input as f64).clamp(0.0, 1.0), self.in_linear) } else { 1.0 };
        let output =
            if self.output > 0 { shape((1.0 - (position - self.out_start) as f64 / self.output as f64).clamp(0.0, 1.0), self.out_linear) } else { 1.0 };
        (input * output) as f32
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decimal_start_rounds_the_binary_value_like_to_fixed() {
        assert_eq!(milliseconds(0.4995), 499);
        assert_eq!(milliseconds(0.5005), 500);
        assert_eq!(milliseconds(1.0005), 1000);
        assert_eq!(milliseconds(0.499500000001), 500);
        assert_eq!(milliseconds(1.9996), 2000);
        assert_eq!(milliseconds(f64::NAN), 0);
    }
    #[test]
    fn simultaneous_linear_edges_multiply_without_block_grid_dependence() {
        let e = Envelope { input: 48000, output: 48000, out_start: 0, in_linear: true, out_linear: true };
        assert_eq!(e.gain(0), 0.0);
        assert_eq!(e.gain(24000), 0.25);
        assert_eq!(e.gain(48000), 0.0);
        assert_eq!(samples(123456, 48000), 5926);
        assert_eq!(samples(5000, 44100), 221);
    }
}
