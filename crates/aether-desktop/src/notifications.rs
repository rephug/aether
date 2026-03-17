use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Manages OS notification cooldowns to avoid spamming the user.
///
/// - "Indexing complete" fires at most once per 5 minutes.
/// - "High drift" fires at most once per module per hour.
/// - "Batch complete" fires at most once per 5 minutes.
#[allow(dead_code)]
pub struct NotificationManager {
    last_indexing_complete: Option<Instant>,
    last_batch_complete: Option<Instant>,
    last_drift_per_module: HashMap<String, Instant>,
    cooldown_indexing: Duration,
    cooldown_drift: Duration,
}

#[allow(dead_code)]
impl NotificationManager {
    pub fn new() -> Self {
        Self {
            last_indexing_complete: None,
            last_batch_complete: None,
            last_drift_per_module: HashMap::new(),
            cooldown_indexing: Duration::from_secs(300),
            cooldown_drift: Duration::from_secs(3600),
        }
    }

    /// Returns true if the notification should fire (cooldown has passed).
    pub fn should_notify_indexing_complete(&mut self) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_indexing_complete
            && now.duration_since(last) < self.cooldown_indexing
        {
            return false;
        }
        self.last_indexing_complete = Some(now);
        true
    }

    /// Returns true if a high-drift notification should fire for a specific module.
    pub fn should_notify_high_drift(&mut self, module: &str) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_drift_per_module.get(module)
            && now.duration_since(*last) < self.cooldown_drift
        {
            return false;
        }
        self.last_drift_per_module.insert(module.to_owned(), now);
        true
    }

    /// Returns true if a batch-complete notification should fire.
    pub fn should_notify_batch_complete(&mut self) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_batch_complete
            && now.duration_since(last) < self.cooldown_indexing
        {
            return false;
        }
        self.last_batch_complete = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldown_prevents_rapid_notifications() {
        let mut mgr = NotificationManager::new();
        assert!(mgr.should_notify_indexing_complete());
        assert!(!mgr.should_notify_indexing_complete());
    }

    #[test]
    fn drift_cooldown_is_per_module() {
        let mut mgr = NotificationManager::new();
        assert!(mgr.should_notify_high_drift("auth"));
        assert!(!mgr.should_notify_high_drift("auth"));
        assert!(mgr.should_notify_high_drift("parser"));
    }

    #[test]
    fn batch_cooldown_works() {
        let mut mgr = NotificationManager::new();
        assert!(mgr.should_notify_batch_complete());
        assert!(!mgr.should_notify_batch_complete());
    }
}
