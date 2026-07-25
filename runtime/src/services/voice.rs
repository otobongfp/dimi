use crate::common::{not_implemented, Result};
use async_trait::async_trait;

#[async_trait]
pub trait VoiceEngine: Send + Sync {
    async fn transcribe(&self, audio: &[u8]) -> Result<String>;
    async fn synthesize(&self, text: &str) -> Result<Vec<u8>>;
}

pub struct StubVoiceEngine;

#[async_trait]
impl VoiceEngine for StubVoiceEngine {
    async fn transcribe(&self, _audio: &[u8]) -> Result<String> {
        not_implemented("VoiceEngine::transcribe (FR9 is V1-optional)")
    }
    async fn synthesize(&self, _text: &str) -> Result<Vec<u8>> {
        not_implemented("VoiceEngine::synthesize (FR9 is V1-optional)")
    }
}
