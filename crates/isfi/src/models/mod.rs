use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDocument {
    pub text: String,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexUpdateReport {
    pub added: usize,
    pub modified: usize,
    pub removed: usize,
    pub renamed: usize,
}
