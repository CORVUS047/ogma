//! Turning a fader position into a gain.
//!
//! A fader that multiplies samples by its own position sounds wrong: hearing is logarithmic, so the
//! top half of such a fader barely changes anything while the bottom half collapses to nothing.
//!
//! The curve here makes the *perceived* loudness follow the fader instead, so half the fader sounds
//! about half as loud. Loudness doubles roughly every 10 dB, which is an amplitude ratio of
//! `10^(10/20) ≈ 3.162`. Writing perceived loudness as `L ∝ A^p`, a doubling of `L` needs
//!
//! ```text
//! 2 = 3.162^p   →   p = ln 2 / ln 3.162 ≈ 0.602
//! ```
//!
//! so amplitude is the fader raised to `1 / 0.602 ≈ 1.661`. At half fader that is `0.5^1.661 ≈
//! 0.316`, which is −10 dB: one halving of loudness, as intended.

/// Exponent that maps a fader position to an amplitude, `1 / 0.602`.
const EXPONENT: f32 = 1.661;

/// The gain a fader at `position` should apply, where `position` runs 0.0 to 1.0.
///
/// A fader at the bottom is silent, not merely quiet, so muting really mutes.
pub fn gain(position: f32) -> f32 {
    let position = position.clamp(0.0, 1.0);

    if position <= 0.0 {
        return 0.0;
    }

    position.powf(EXPONENT)
}

/// The fader position that produces `gain`, the inverse of [`gain`].
///
/// Useful for setting a fader from a level expressed as an amplitude, and for checking the curve.
pub fn position_from_gain(gain: f32) -> f32 {
    let gain = gain.clamp(0.0, 1.0);

    if gain <= 0.0 {
        return 0.0;
    }

    gain.powf(1.0 / EXPONENT)
}

/// How far down from full scale a fader at `position` sits, in decibels.
///
/// `None` at the bottom of the fader, where the level is silence rather than a number of decibels.
pub fn decibels(position: f32) -> Option<f32> {
    let gain = gain(position);

    (gain > 0.0).then(|| 20.0 * gain.log10())
}

/// A fader position as a percentage, for display.
pub fn percent(position: f32) -> u32 {
    (position.clamp(0.0, 1.0) * 100.0).round() as u32
}

/// A fader position as a decibel reading, for display: `-10.0 dB`, or `muted` at the bottom.
pub fn display_decibels(position: f32) -> String {
    match decibels(position) {
        Some(db) => format!("{db:.1} dB"),
        None => "muted".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ends_of_the_fader_are_silence_and_full_scale() {
        assert_eq!(gain(0.0), 0.0);
        assert_eq!(gain(1.0), 1.0);

        // Out of range positions are clamped rather than producing nonsense.
        assert_eq!(gain(-1.0), 0.0);
        assert_eq!(gain(5.0), 1.0);
    }

    #[test]
    fn half_the_fader_is_half_the_loudness() {
        // Half perceived loudness is ten decibels down.
        let db = decibels(0.5).expect("a level");

        assert!((db + 10.0).abs() < 0.1, "expected about -10 dB, got {db}");
    }

    #[test]
    fn each_halving_of_the_fader_drops_another_ten_decibels() {
        let steps = [0.5, 0.25, 0.125];

        for (index, position) in steps.iter().enumerate() {
            let expected = -10.0 * (index as f32 + 1.0);
            let db = decibels(*position).expect("a level");

            assert!(
                (db - expected).abs() < 0.2,
                "at {position} expected about {expected} dB, got {db}"
            );
        }
    }

    #[test]
    fn the_curve_only_ever_rises() {
        let mut previous = gain(0.0);

        for step in 1..=100 {
            let gain = gain(step as f32 / 100.0);

            assert!(gain > previous, "gain fell at {step}: {gain} after {previous}");
            previous = gain;
        }
    }

    #[test]
    fn the_curve_is_gentler_than_a_plain_multiply_everywhere_below_full() {
        // The point of the curve: a given fader position is quieter than a naive fader, which is
        // what leaves the useful range spread across the whole travel.
        for step in 1..100 {
            let position = step as f32 / 100.0;

            assert!(gain(position) < position, "at {position}");
        }
    }

    #[test]
    fn position_and_gain_convert_back_and_forth() {
        for step in 0..=20 {
            let position = step as f32 / 20.0;
            let round_trip = position_from_gain(gain(position));

            assert!((round_trip - position).abs() < 1e-4, "{position} became {round_trip}");
        }
    }

    #[test]
    fn readings_are_formatted_for_people() {
        assert_eq!(percent(0.5), 50);
        assert_eq!(display_decibels(1.0), "0.0 dB");
        assert_eq!(display_decibels(0.0), "muted");
        assert_eq!(display_decibels(0.5), "-10.0 dB");
    }
}
