use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Registered,
    Initializing,
    Ready,
    Degraded,
    Stopped,
    Failed,
}

#[derive(Default)]
pub struct LifecycleTracker {
    states: RwLock<HashMap<String, LifecycleState>>,
}

impl LifecycleTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, name: &str, state: LifecycleState) {
        self.states
            .write()
            .expect("lifecycle tracker lock poisoned")
            .insert(name.to_string(), state);
    }

    pub fn get(&self, name: &str) -> Option<LifecycleState> {
        self.states
            .read()
            .expect("lifecycle tracker lock poisoned")
            .get(name)
            .copied()
    }

    pub fn all(&self) -> HashMap<String, LifecycleState> {
        self.states
            .read()
            .expect("lifecycle tracker lock poisoned")
            .clone()
    }
}
