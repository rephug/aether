use serde::{Deserialize, Serialize};

/// Training sample for aftershock logistic regression.
#[derive(Debug, Clone)]
pub struct TrainingSample {
    pub delta_sem_source: f64,
    pub coupling: f64,
    pub graph_distance: f64,
    pub pagerank_target: f64,
    pub target_breached: bool,
}

/// Prediction result for a single downstream symbol.
#[derive(Debug, Clone)]
pub struct AftershockPrediction {
    pub target_symbol_id: String,
    pub probability: f64,
}

/// Minimal logistic regression model for aftershock prediction.
///
/// ```text
/// P(Δ_sem_A > τ) = σ(w₀ + w₁·Δ_sem_B + w₂·C_AB + w₃·γ + w₄·PR_A)
/// ```
///
/// Where C_AB = coupling score, γ = graph distance, σ = logistic sigmoid.
/// Weights trained via pure-Rust gradient descent (no external ML crate needed
/// for 5 parameters).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AftershockModel {
    pub weights: [f64; 5],
    pub trained_at: i64,
}

impl AftershockModel {
    /// Predict the probability that a downstream symbol will breach the noise floor.
    pub fn predict(&self, delta_sem_b: f64, coupling: f64, distance: f64, pagerank_a: f64) -> f64 {
        let z = self.weights[0]
            + self.weights[1] * delta_sem_b
            + self.weights[2] * coupling
            + self.weights[3] * distance
            + self.weights[4] * pagerank_a;
        sigmoid(z)
    }
}

/// Train a logistic regression model using gradient descent.
///
/// Uses binary cross-entropy loss with a fixed learning rate and iteration count.
/// This is intentionally simple — we only have 5 weights and training data is
/// relatively small (bounded by fingerprint history × neighbor pairs).
pub fn train(data: &[TrainingSample], learning_rate: f64, iterations: usize) -> AftershockModel {
    let mut weights = [0.0_f64; 5];

    if data.is_empty() {
        return AftershockModel {
            weights,
            trained_at: 0,
        };
    }

    for _ in 0..iterations {
        let mut gradients = [0.0_f64; 5];

        for sample in data {
            let features = [
                1.0, // bias
                sample.delta_sem_source,
                sample.coupling,
                sample.graph_distance,
                sample.pagerank_target,
            ];

            let z: f64 = weights
                .iter()
                .zip(features.iter())
                .map(|(w, x)| w * x)
                .sum();
            let predicted = sigmoid(z);
            let target = if sample.target_breached { 1.0 } else { 0.0 };
            let error = predicted - target;

            for (g, x) in gradients.iter_mut().zip(features.iter()) {
                *g += error * x;
            }
        }

        let n = data.len() as f64;
        for (w, g) in weights.iter_mut().zip(gradients.iter()) {
            *w -= learning_rate * g / n;
        }
    }

    AftershockModel {
        weights,
        trained_at: 0, // Caller sets this
    }
}

/// Compute approximate AUC-ROC for a trained model on the given data.
/// Uses the Mann-Whitney U statistic interpretation of AUC.
pub fn compute_auc_roc(model: &AftershockModel, data: &[TrainingSample]) -> Option<f64> {
    if data.is_empty() {
        return None;
    }

    let mut positive_scores = Vec::new();
    let mut negative_scores = Vec::new();

    for sample in data {
        let score = model.predict(
            sample.delta_sem_source,
            sample.coupling,
            sample.graph_distance,
            sample.pagerank_target,
        );
        if sample.target_breached {
            positive_scores.push(score);
        } else {
            negative_scores.push(score);
        }
    }

    if positive_scores.is_empty() || negative_scores.is_empty() {
        return None;
    }

    let mut concordant = 0_usize;
    let mut tied = 0_usize;
    let total = positive_scores.len() * negative_scores.len();

    for &pos in &positive_scores {
        for &neg in &negative_scores {
            if pos > neg {
                concordant += 1;
            } else if (pos - neg).abs() < 1e-12 {
                tied += 1;
            }
        }
    }

    Some((concordant as f64 + 0.5 * tied as f64) / total as f64)
}

fn sigmoid(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigmoid_output_bounded() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-10);
        assert!(sigmoid(100.0) > 0.99);
        assert!(sigmoid(-100.0) < 0.01);
        // Extreme values should not NaN
        assert!(sigmoid(1000.0).is_finite());
        assert!(sigmoid(-1000.0).is_finite());
    }

    #[test]
    fn training_converges() {
        // Create linearly separable synthetic data:
        // High delta_sem + high coupling → breaches (positive)
        // Low delta_sem + low coupling → doesn't breach (negative)
        let data: Vec<TrainingSample> = (0..100)
            .map(|i| {
                if i < 50 {
                    TrainingSample {
                        delta_sem_source: 0.8,
                        coupling: 0.9,
                        graph_distance: 1.0,
                        pagerank_target: 0.5,
                        target_breached: true,
                    }
                } else {
                    TrainingSample {
                        delta_sem_source: 0.05,
                        coupling: 0.1,
                        graph_distance: 3.0,
                        pagerank_target: 0.01,
                        target_breached: false,
                    }
                }
            })
            .collect();

        let model = train(&data, 0.01, 1000);

        // Model should predict high probability for positive-like inputs
        let p_positive = model.predict(0.8, 0.9, 1.0, 0.5);
        let p_negative = model.predict(0.05, 0.1, 3.0, 0.01);

        assert!(
            p_positive > p_negative,
            "positive prediction ({p_positive}) should be > negative ({p_negative})"
        );
        assert!(p_positive > 0.5, "positive prediction should be > 0.5");
        assert!(p_negative < 0.5, "negative prediction should be < 0.5");
    }

    #[test]
    fn train_empty_data_returns_zero_weights() {
        let model = train(&[], 0.01, 100);
        assert_eq!(model.weights, [0.0; 5]);
    }

    #[test]
    fn auc_roc_perfect_separation() {
        let model = AftershockModel {
            weights: [0.0, 10.0, 0.0, 0.0, 0.0],
            trained_at: 0,
        };
        let data = vec![
            TrainingSample {
                delta_sem_source: 1.0,
                coupling: 0.0,
                graph_distance: 0.0,
                pagerank_target: 0.0,
                target_breached: true,
            },
            TrainingSample {
                delta_sem_source: -1.0,
                coupling: 0.0,
                graph_distance: 0.0,
                pagerank_target: 0.0,
                target_breached: false,
            },
        ];
        let auc = compute_auc_roc(&model, &data).unwrap();
        assert!((auc - 1.0).abs() < 1e-10);
    }
}
