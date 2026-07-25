use crate::common::{Result, SqlValue};
use crate::services::storage::StorageEngine;
use std::sync::Arc;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub async fn record(
    storage: &Arc<dyn StorageEngine>,
    actor: &str,
    action: &str,
    subject: Option<&str>,
) -> Result<()> {
    storage
        .query(
            "INSERT INTO audit_log (id, actor, action, subject, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            &[
                SqlValue::Text(uuid::Uuid::new_v4().to_string()),
                SqlValue::Text(actor.to_string()),
                SqlValue::Text(action.to_string()),
                match subject {
                    Some(s) => SqlValue::Text(s.to_string()),
                    None => SqlValue::Null,
                },
                SqlValue::Integer(now_unix()),
            ],
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::storage::SqliteStorageEngine;

    #[tokio::test]
    async fn writes_a_row() {
        let storage: Arc<dyn StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        record(
            &storage,
            "user",
            "plugin.installed",
            Some("dimi-test-plugin"),
        )
        .await
        .unwrap();

        let rows = storage
            .query("SELECT actor, action, subject FROM audit_log", &[])
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].0.get("action"),
            Some(&SqlValue::Text("plugin.installed".to_string()))
        );
    }
}
