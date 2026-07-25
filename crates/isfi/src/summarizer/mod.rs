use crate::models::ParsedDocument;
use async_trait::async_trait;

#[async_trait]
pub trait Summarizer: Send + Sync {
    async fn summarize(&self, document: &ParsedDocument) -> Result<String, crate::Error>;
}
