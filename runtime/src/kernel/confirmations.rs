use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

/// Lets one Tauri command (the chat-turn loop, paused mid-flight on a
/// destructive tool call) wait on a decision that arrives via a second,
/// independent command (`respond_to_tool_confirmation`) — the same
/// one-command-blocks/one-command-unblocks shape as `SystemCheckState`'s
/// `continue_notify`, generalized to a keyed map since multiple confirmations
/// can be in flight at once (different conversations, or a fast-typing user
/// queuing another destructive call right after approving the first).
pub struct PendingConfirmations {
    waiters: Mutex<HashMap<Uuid, oneshot::Sender<bool>>>,
}

impl PendingConfirmations {
    pub fn new() -> Self {
        Self {
            waiters: Mutex::new(HashMap::new()),
        }
    }

    pub fn register(&self) -> (Uuid, oneshot::Receiver<bool>) {
        let id = Uuid::new_v4();
        let (tx, rx) = oneshot::channel();
        self.waiters
            .lock()
            .expect("pending confirmations lock poisoned")
            .insert(id, tx);
        (id, rx)
    }

    /// Resolves a pending confirmation. Returns `false` if `id` has no live
    /// waiter (already resolved, timed out, or from a previous app session).
    pub fn resolve(&self, id: Uuid, approved: bool) -> bool {
        let sender = self
            .waiters
            .lock()
            .expect("pending confirmations lock poisoned")
            .remove(&id);
        match sender {
            Some(tx) => tx.send(approved).is_ok(),
            None => false,
        }
    }

    /// Waits for a decision, defaulting to denied on timeout — fail-closed
    /// is correct for a destructive-action gate. Always removes `id` from
    /// the map afterward regardless of outcome, so a timed-out/abandoned
    /// confirmation can't leak a dead entry forever (the timeout path never
    /// goes through `resolve`, so cleanup can't be left as a caller
    /// obligation — it has to happen here).
    pub async fn wait(&self, id: Uuid, rx: oneshot::Receiver<bool>, timeout: Duration) -> bool {
        let approved = tokio::time::timeout(timeout, rx)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or(false);
        self.waiters
            .lock()
            .expect("pending confirmations lock poisoned")
            .remove(&id);
        approved
    }
}

impl Default for PendingConfirmations {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn approve_resolves_the_wait_with_true() {
        let confirmations = PendingConfirmations::new();
        let (id, rx) = confirmations.register();
        let confirmations = std::sync::Arc::new(confirmations);
        let waiter = {
            let confirmations = confirmations.clone();
            tokio::spawn(async move { confirmations.wait(id, rx, Duration::from_secs(5)).await })
        };
        assert!(confirmations.resolve(id, true));
        assert!(waiter.await.unwrap());
    }

    #[tokio::test]
    async fn deny_resolves_the_wait_with_false() {
        let confirmations = PendingConfirmations::new();
        let (id, rx) = confirmations.register();
        let confirmations = std::sync::Arc::new(confirmations);
        let waiter = {
            let confirmations = confirmations.clone();
            tokio::spawn(async move { confirmations.wait(id, rx, Duration::from_secs(5)).await })
        };
        assert!(confirmations.resolve(id, false));
        assert!(!waiter.await.unwrap());
    }

    #[tokio::test]
    async fn timeout_defaults_to_denied_and_cleans_up_the_map() {
        let confirmations = PendingConfirmations::new();
        let (id, rx) = confirmations.register();
        let approved = confirmations.wait(id, rx, Duration::from_millis(10)).await;
        assert!(!approved);
        // Resolving after timeout finds no waiter left — proves the map
        // entry didn't leak.
        assert!(!confirmations.resolve(id, true));
    }

    #[tokio::test]
    async fn resolving_an_unknown_id_is_a_no_op() {
        let confirmations = PendingConfirmations::new();
        assert!(!confirmations.resolve(Uuid::new_v4(), true));
    }
}
