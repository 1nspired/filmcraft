//! Decimal rounding of a bounded nonnegative binary64 rational, without an
//! intermediate binary multiplication that changes exact half-way cases.
pub(crate) fn rounded_positive(value: f64, scale: u32) -> i64 {
    if !value.is_finite() || value <= 0.0 || scale == 0 {
        return 0;
    }
    if value >= i64::MAX as f64 / f64::from(scale) {
        return i64::MAX;
    }
    let bits = value.to_bits();
    let power = ((bits >> 52) & 0x7ff) as i32 - 1023 - 52;
    let numerator = (((bits & ((1u64 << 52) - 1)) | (1u64 << 52)) as u128) * u128::from(scale);
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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_balance_and_fade_start_keep_binary_half_cases() {
        assert_eq!(rounded_positive(0.4995, 1000), 499);
        assert_eq!(rounded_positive(0.5005, 1000), 500);
        assert_eq!(rounded_positive(0.99995, 10000), 10000);
        assert_eq!(rounded_positive(0.12345, 10000), 1235);
        assert_eq!(rounded_positive(f64::MAX, 10000), i64::MAX);
        assert_eq!(rounded_positive(f64::NAN, 1000), 0);
    }
}
