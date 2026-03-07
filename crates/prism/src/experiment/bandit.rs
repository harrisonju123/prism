use std::collections::HashMap;

/// Per-variant Beta distribution parameters.
#[derive(Debug, Clone)]
pub struct ArmState {
    pub alpha: f64,
    pub beta: f64,
}

impl Default for ArmState {
    fn default() -> Self {
        Self {
            alpha: 1.0,
            beta: 1.0,
        }
    }
}

/// Thompson Sampling multi-armed bandit state.
#[derive(Debug, Clone, Default)]
pub struct BanditState {
    pub arms: HashMap<String, ArmState>,
}

impl BanditState {
    /// Sample from each arm's Beta distribution and return the name with the highest sample.
    pub fn sample_best(&self, variant_names: &[String]) -> String {
        let mut rng = rand::rng();
        let mut best_name = variant_names[0].clone();
        let mut best_sample = f64::NEG_INFINITY;

        for name in variant_names {
            let arm = self.arms.get(name).cloned().unwrap_or_default();
            let sample = sample_beta(&mut rng, arm.alpha, arm.beta);
            if sample > best_sample {
                best_sample = sample;
                best_name = name.clone();
            }
        }

        best_name
    }

    /// Update arm state with a reward observation.
    /// reward should be in [0, 1].
    pub fn update(&mut self, arm: &str, reward: f64) {
        let state = self.arms.entry(arm.to_string()).or_default();
        state.alpha += reward;
        state.beta += 1.0 - reward;
    }
}

/// Sample from Beta(alpha, beta) using the Gamma distribution trick:
/// Beta(a, b) = X / (X + Y) where X ~ Gamma(a, 1) and Y ~ Gamma(b, 1).
///
/// Uses the Marsaglia-Tsang method for Gamma sampling.
fn sample_beta(rng: &mut impl rand::Rng, alpha: f64, beta: f64) -> f64 {
    let x = sample_gamma(rng, alpha);
    let y = sample_gamma(rng, beta);
    if x + y == 0.0 { 0.5 } else { x / (x + y) }
}

/// Marsaglia-Tsang method for Gamma(shape, 1) sampling.
/// For shape < 1, uses the transformation: Gamma(a) = Gamma(a+1) * U^(1/a).
fn sample_gamma(rng: &mut impl rand::Rng, shape: f64) -> f64 {
    if shape < 1.0 {
        // Gamma(a) = Gamma(a+1) * U^(1/a)
        let u: f64 = rng.random();
        return sample_gamma(rng, shape + 1.0) * u.powf(1.0 / shape);
    }

    let d = shape - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();

    loop {
        let x: f64 = sample_standard_normal(rng);
        let v = (1.0 + c * x).powi(3);
        if v <= 0.0 {
            continue;
        }
        let u: f64 = rng.random();
        let x2 = x * x;

        if u < 1.0 - 0.0331 * x2 * x2 {
            return d * v;
        }
        if u.ln() < 0.5 * x2 + d * (1.0 - v + v.ln()) {
            return d * v;
        }
    }
}

/// Box-Muller transform for standard normal sampling.
fn sample_standard_normal(rng: &mut impl rand::Rng) -> f64 {
    let u1: f64 = rng.random();
    let u2: f64 = rng.random();
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beta_sampling_bounds() {
        let mut rng = rand::rng();
        for _ in 0..1000 {
            let sample = sample_beta(&mut rng, 1.0, 1.0);
            assert!(
                (0.0..=1.0).contains(&sample),
                "sample out of bounds: {sample}"
            );
        }
    }

    #[test]
    fn strong_prior_bias() {
        // Beta(100, 1) should almost always produce values near 1.0
        let mut rng = rand::rng();
        let mut sum = 0.0;
        let n = 1000;
        for _ in 0..n {
            sum += sample_beta(&mut rng, 100.0, 1.0);
        }
        let mean = sum / n as f64;
        assert!(mean > 0.9, "expected mean > 0.9, got {mean}");
    }

    #[test]
    fn sample_best_winner_selection() {
        let mut state = BanditState::default();
        // Give arm "winner" a very strong prior
        state.arms.insert(
            "winner".to_string(),
            ArmState {
                alpha: 100.0,
                beta: 1.0,
            },
        );
        state.arms.insert(
            "loser".to_string(),
            ArmState {
                alpha: 1.0,
                beta: 100.0,
            },
        );

        let variants = vec!["winner".to_string(), "loser".to_string()];
        let mut winner_count = 0;
        for _ in 0..100 {
            if state.sample_best(&variants) == "winner" {
                winner_count += 1;
            }
        }
        assert!(
            winner_count > 90,
            "expected winner to be selected >90/100 times, got {winner_count}"
        );
    }

    #[test]
    fn update_math() {
        let mut state = BanditState::default();
        state.update("arm1", 1.0);
        let arm = state.arms.get("arm1").unwrap();
        assert!((arm.alpha - 2.0).abs() < f64::EPSILON); // 1.0 + 1.0
        assert!((arm.beta - 1.0).abs() < f64::EPSILON); // 1.0 + 0.0

        state.update("arm1", 0.0);
        let arm = state.arms.get("arm1").unwrap();
        assert!((arm.alpha - 2.0).abs() < f64::EPSILON); // 2.0 + 0.0
        assert!((arm.beta - 2.0).abs() < f64::EPSILON); // 1.0 + 1.0
    }

    #[test]
    fn default_state() {
        let state = BanditState::default();
        assert!(state.arms.is_empty());

        let arm = ArmState::default();
        assert!((arm.alpha - 1.0).abs() < f64::EPSILON);
        assert!((arm.beta - 1.0).abs() < f64::EPSILON);
    }
}
