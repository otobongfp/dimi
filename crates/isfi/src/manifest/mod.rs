use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub root_id: String,
    pub nodes: HashMap<String, Node>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeType {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub path: PathBuf,
    pub node_type: NodeType,
    pub checksum: String,
    pub modified: u64,
    pub parent_id: Option<String>,
    pub children: Vec<String>,
}

pub struct ManifestDiff {
    pub added: Vec<Node>,
    pub modified: Vec<Node>,
    pub removed: Vec<String>,
}

impl Manifest {
    pub fn new(root_id: String) -> Self {
        Self {
            version: 1,
            root_id,
            nodes: HashMap::new(),
        }
    }

    pub fn diff(&self, new_manifest: &Manifest) -> ManifestDiff {
        let mut added = Vec::new();
        let mut modified = Vec::new();
        let mut removed = Vec::new();

        for (id, new_node) in &new_manifest.nodes {
            if let Some(old_node) = self.nodes.get(id) {
                if old_node.checksum != new_node.checksum {
                    modified.push(new_node.clone());
                }
            } else {
                added.push(new_node.clone());
            }
        }

        for id in self.nodes.keys() {
            if !new_manifest.nodes.contains_key(id) {
                removed.push(id.clone());
            }
        }

        ManifestDiff {
            added,
            modified,
            removed,
        }
    }
}
