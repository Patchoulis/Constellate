//! A small seeded random generator, so that every result worked out from it is the same on every run. (Its numbers
//! are the same on every computer too; what is computed from them can still differ in the last bits between
//! processors, whose matrix products use different instructions.)
//!
//! It is SplitMix64 (Steele, Lea and Flood, 2014): one 64-bit state, a fixed step, and a mixing function. It is
//! not for secrets; it is for choosing start vectors and k-means seeds, where only evenness and repeatability
//! matter, and a few lines written here are easier to keep identical than a crate whose sequence may change.

/// What the state moves by at every draw.
const STEP: u64 = 0x9E37_79B9_7F4A_7C15;

/// A random generator whose whole sequence is decided by its seed.
#[derive(Clone, Debug)]
pub struct Random {
    state: u64,
}

impl Random {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(STEP);
        let mut mixed = self.state;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ (mixed >> 31)
    }

    /// A number in `[0, 1)`, evenly spread: the top 53 bits, which is all a 64-bit float can hold.
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1_u64 << 53) as f64
    }

    /// A number from the standard normal distribution (Box-Muller; the second number it makes is not kept).
    pub fn normal(&mut self) -> f64 {
        // 1 - uniform is in (0, 1], so the logarithm is finite
        let radius = (-2.0 * (1.0 - self.uniform()).ln()).sqrt();
        radius * (std::f64::consts::TAU * self.uniform()).cos()
    }

    /// Move on as `draws` calls of [`Random::next_u64`] would (each of the numbers here draws once), at once: the
    /// state only ever moves by a fixed step. Work done side by side on threads can so start each piece where it
    /// would have started had the pieces been done one after another.
    pub fn skip(&mut self, draws: u64) {
        self.state = self.state.wrapping_add(STEP.wrapping_mul(draws));
    }

    /// An index in `0..count`, each as likely as the others. `count` must not be 0.
    pub fn below(&mut self, count: usize) -> usize {
        ((self.uniform() * count as f64) as usize).min(count.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_sequence_and_another_seed_another() {
        let first: Vec<u64> = (0..5).scan(Random::new(7), |random, _| Some(random.next_u64())).collect();
        let again: Vec<u64> = (0..5).scan(Random::new(7), |random, _| Some(random.next_u64())).collect();
        let other: Vec<u64> = (0..5).scan(Random::new(8), |random, _| Some(random.next_u64())).collect();
        assert_eq!(first, again);
        assert_ne!(first, other);
    }

    #[test]
    fn skipping_draws_lands_where_drawing_them_would() {
        let mut drawn = Random::new(11);
        let mut skipped = Random::new(11);
        for _ in 0..1_234 {
            drawn.uniform();
        }
        skipped.skip(1_234);
        assert_eq!(drawn.next_u64(), skipped.next_u64());
        skipped.skip(0);
        assert_eq!(drawn.normal().to_bits(), skipped.normal().to_bits()); // a normal draws twice
    }

    #[test]
    fn uniform_numbers_stay_in_range_and_average_a_half() {
        let mut random = Random::new(1);
        let numbers: Vec<f64> = (0..100_000).map(|_| random.uniform()).collect();
        assert!(numbers.iter().all(|number| (0.0..1.0).contains(number)));
        let mean = numbers.iter().sum::<f64>() / numbers.len() as f64;
        assert!((mean - 0.5).abs() < 0.01, "{mean}");
    }

    #[test]
    fn normal_numbers_have_mean_zero_and_spread_one() {
        let mut random = Random::new(2);
        let numbers: Vec<f64> = (0..100_000).map(|_| random.normal()).collect();
        let mean = numbers.iter().sum::<f64>() / numbers.len() as f64;
        let variance = numbers.iter().map(|number| (number - mean).powi(2)).sum::<f64>() / numbers.len() as f64;
        assert!(mean.abs() < 0.02, "{mean}");
        assert!((variance - 1.0).abs() < 0.03, "{variance}");
    }

    #[test]
    fn an_index_below_a_count_reaches_every_value_and_no_further() {
        let mut random = Random::new(3);
        let mut seen = [0_usize; 4];
        for _ in 0..4_000 {
            seen[random.below(4)] += 1;
        }
        assert!(seen.iter().all(|times| *times > 800), "{seen:?}");
        assert_eq!(Random::new(4).below(1), 0);
    }
}
