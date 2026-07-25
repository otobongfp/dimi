use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::io::AsyncReadExt;

pub struct ChecksumEngine;

impl ChecksumEngine {
    pub async fn hash_file(path: &Path) -> Result<String, crate::Error> {
        let mut file = tokio::fs::File::open(path).await?;
        let mut hasher = Sha256::new();
        let mut buffer = [0; 8192];

        loop {
            let count = file.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }

        Ok(hex::encode(hasher.finalize()))
    }
}
