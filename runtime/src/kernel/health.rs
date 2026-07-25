use crate::kernel::events::{topics, EventBus};
use crate::kernel::lifecycle::{LifecycleState, LifecycleTracker};
use std::collections::HashMap;
use std::sync::RwLock;

const MAX_CONSECUTIVE_MISSES: u32 = 3;

#[derive(Default)]
pub struct HealthMonitor {
    misses: RwLock<HashMap<String, u32>>,
}

impl HealthMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, name: &str, healthy: bool, tracker: &LifecycleTracker, events: &EventBus) {
        let mut misses = self.misses.write().expect("health monitor lock poisoned");
        if healthy {
            misses.insert(name.to_string(), 0);
            return;
        }
        let count = misses.entry(name.to_string()).or_insert(0);
        *count += 1;
        if *count >= MAX_CONSECUTIVE_MISSES {
            tracker.set(name, LifecycleState::Degraded);
            events.publish(
                topics::HEALTH_DEGRADED,
                serde_json::json!({ "service": name, "consecutive_misses": *count }),
            );
        }
    }
}
