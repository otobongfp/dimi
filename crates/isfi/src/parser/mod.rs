use crate::models::ParsedDocument;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use tokio::fs;

#[async_trait]
pub trait DocumentParser: Send + Sync {
    fn accepts(&self, path: &Path) -> bool;

    async fn parse(&self, path: &Path) -> Result<ParsedDocument, crate::Error>;
}

pub struct ParserRegistry {
    parsers: Vec<Box<dyn DocumentParser>>,
}

impl Default for ParserRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ParserRegistry {
    pub fn new() -> Self {
        Self {
            parsers: Vec::new(),
        }
    }

    pub fn register(&mut self, parser: Box<dyn DocumentParser>) {
        self.parsers.push(parser);
    }

    pub async fn parse(&self, path: &Path) -> Result<Option<ParsedDocument>, crate::Error> {
        for parser in &self.parsers {
            if parser.accepts(path) {
                return Ok(Some(parser.parse(path).await?));
            }
        }
        Ok(None)
    }
}

pub struct TextParser;

#[async_trait]
impl DocumentParser for TextParser {
    fn accepts(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            matches!(
                ext.to_lowercase().as_str(),
                "txt" | "md" | "csv" | "json" | "rs" | "ts" | "js"
            )
        } else {
            false
        }
    }

    async fn parse(&self, path: &Path) -> Result<ParsedDocument, crate::Error> {
        let text = fs::read_to_string(path).await?;

        let mut metadata = HashMap::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            metadata.insert("extension".to_string(), ext.to_string());
        }

        Ok(ParsedDocument { text, metadata })
    }
}
