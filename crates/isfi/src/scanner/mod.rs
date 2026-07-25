use std::path::{Path, PathBuf};
use tokio::fs;

pub struct Scanner;

#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
}

impl Scanner {
    pub async fn scan_dir(root: &Path) -> Result<Vec<ScannedFile>, crate::Error> {
        let mut results = Vec::new();
        let mut dirs_to_visit = vec![root.to_path_buf()];

        while let Some(dir) = dirs_to_visit.pop() {
            let mut entries = match fs::read_dir(&dir).await {
                Ok(e) => e,
                Err(_) => continue,
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let file_name = entry.file_name().to_string_lossy().to_string();

                if file_name.starts_with('.') {
                    continue;
                }

                let Ok(metadata) = fs::metadata(&path).await else {
                    continue;
                };

                if metadata.is_dir() {
                    if matches!(
                        file_name.as_str(),
                        "node_modules" | "target" | "build" | "dist" | "vendor" | "out"
                    ) {
                        continue;
                    }
                    dirs_to_visit.push(path);
                } else if metadata.is_file() {
                    results.push(ScannedFile { path });
                }
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scan_dir_follows_symlinked_files() {
        let root = std::env::temp_dir().join(format!("isfi-scanner-test-{}", std::process::id()));
        let real_dir = root.join("real");
        let linked_dir = root.join("linked");
        tokio::fs::create_dir_all(&real_dir).await.unwrap();
        tokio::fs::create_dir_all(&linked_dir).await.unwrap();

        let real_file = real_dir.join("note.txt");
        tokio::fs::write(&real_file, b"hello").await.unwrap();

        let symlinked_file = linked_dir.join("note.txt");
        std::os::unix::fs::symlink(&real_file, &symlinked_file).unwrap();

        let found = Scanner::scan_dir(&linked_dir).await.unwrap();

        std::fs::remove_dir_all(&root).ok();

        assert_eq!(found.len(), 1, "expected the symlinked file to be scanned, found: {found:?}");
        assert_eq!(found[0].path, symlinked_file);
    }
}
