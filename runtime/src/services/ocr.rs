use crate::common::{not_implemented, Result};
use async_trait::async_trait;

#[async_trait]
pub trait OcrEngine: Send + Sync {
    async fn recognize(&self, image_bytes: &[u8]) -> Result<String>;
    fn is_available(&self) -> bool;
}

pub struct StubOcrEngine;

#[async_trait]
impl OcrEngine for StubOcrEngine {
    async fn recognize(&self, _image_bytes: &[u8]) -> Result<String> {
        not_implemented("OcrEngine::recognize")
    }
    fn is_available(&self) -> bool {
        false
    }
}

use crate::common::DimiError;

pub struct TesseractOcrEngine {
    available: bool,
}

impl TesseractOcrEngine {
    pub fn new() -> Self {
        let available = tesseract::Tesseract::new(None, Some("eng")).is_ok();
        Self { available }
    }
}

impl Default for TesseractOcrEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OcrEngine for TesseractOcrEngine {
    async fn recognize(&self, image_bytes: &[u8]) -> Result<String> {
        if !self.available {
            return Err(DimiError::Degraded(
                "OCR unavailable: Tesseract is not installed on this machine".into(),
            ));
        }
        let bytes = image_bytes.to_vec();
        tokio::task::spawn_blocking(move || run_tesseract(&bytes))
            .await
            .map_err(|e| DimiError::Internal(format!("OCR task panicked: {e}")))?
    }

    fn is_available(&self) -> bool {
        self.available
    }
}

fn run_tesseract(image_bytes: &[u8]) -> Result<String> {
    let tess = tesseract::Tesseract::new(None, Some("eng"))
        .map_err(|e| DimiError::Internal(format!("tesseract init failed: {e}")))?;
    let mut tess = tess
        .set_image_from_mem(image_bytes)
        .map_err(|e| DimiError::Internal(format!("tesseract set_image failed: {e}")))?;
    tess.get_text()
        .map_err(|e| DimiError::Internal(format!("tesseract recognize failed: {e}")))
}
