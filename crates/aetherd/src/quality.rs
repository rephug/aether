use std::collections::VecDeque;

use tracing::warn;

#[derive(Debug)]
pub struct SirQualityMonitor {
    recent_confidences: VecDeque<f32>,
    window_size: usize,
    floor: f32,
    warned_for_current_dip: bool,
    /// False for providers whose low confidence is intentional (the mock provider's
    /// placeholders), so the floor warning never fires for them.
    enabled: bool,
}

impl SirQualityMonitor {
    pub fn new(window_size: usize, floor: f32) -> Self {
        Self {
            recent_confidences: VecDeque::with_capacity(window_size.max(1)),
            window_size: window_size.max(1),
            floor,
            warned_for_current_dip: false,
            enabled: true,
        }
    }

    /// A monitor that records nothing and never warns.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::new(1, 0.0)
        }
    }

    pub fn record(&mut self, confidence: f32) -> bool {
        if !self.enabled {
            return false;
        }
        self.recent_confidences.push_back(confidence);
        if self.recent_confidences.len() > self.window_size {
            self.recent_confidences.pop_front();
        }

        let Some(avg_confidence) = self.rolling_average() else {
            return false;
        };

        if avg_confidence < self.floor {
            if self.warned_for_current_dip {
                return false;
            }

            self.warned_for_current_dip = true;
            warn!(
                avg_confidence = avg_confidence,
                floor = self.floor,
                window_size = self.window_size,
                "SIR quality is low (avg confidence {:.2}); the configured [inference] provider or model may be too weak for this codebase.",
                avg_confidence
            );
            return true;
        }

        self.warned_for_current_dip = false;
        false
    }

    fn rolling_average(&self) -> Option<f32> {
        if self.recent_confidences.len() < self.window_size {
            return None;
        }

        let sum = self
            .recent_confidences
            .iter()
            .fold(0.0f32, |acc, value| acc + value);
        Some(sum / self.recent_confidences.len() as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::SirQualityMonitor;

    #[test]
    fn disabled_monitor_never_warns() {
        let mut monitor = SirQualityMonitor::disabled();
        for _ in 0..20 {
            assert!(!monitor.record(0.1));
        }
    }

    #[test]
    fn warns_after_window_of_low_confidence_results() {
        let mut monitor = SirQualityMonitor::new(3, 0.3);

        assert!(!monitor.record(0.2));
        assert!(!monitor.record(0.1));
        assert!(monitor.record(0.2));
        assert!(!monitor.record(0.25));
    }

    #[test]
    fn resets_warning_state_after_quality_recovers() {
        let mut monitor = SirQualityMonitor::new(3, 0.3);

        assert!(!monitor.record(0.2));
        assert!(!monitor.record(0.2));
        assert!(monitor.record(0.2));
        assert!(!monitor.record(0.9));
        assert!(!monitor.record(0.1));
        assert!(!monitor.record(0.1));
        assert!(monitor.record(0.0));
    }

    #[test]
    fn does_not_warn_before_window_is_full() {
        let mut monitor = SirQualityMonitor::new(4, 0.3);

        assert!(!monitor.record(0.1));
        assert!(!monitor.record(0.1));
        assert!(!monitor.record(0.1));
    }
}
