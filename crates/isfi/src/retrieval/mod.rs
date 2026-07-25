use crate::api::ContextBundle;
use crate::manifest::Node;
use std::collections::HashMap;

pub struct ContextBuilder;

impl ContextBuilder {
    pub fn build(
        library_summary: String,
        folder_nodes: Vec<Node>,
        file_nodes: Vec<Node>,
        summaries: HashMap<String, String>,
    ) -> ContextBundle {
        let mut folder_summaries = Vec::new();
        for folder in folder_nodes {
            if let Some(summary) = summaries.get(&folder.id) {
                folder_summaries.push(format!("{}: {}", folder.path.display(), summary));
            }
        }

        let mut relevant_files = Vec::new();
        for file in file_nodes {
            relevant_files.push(file.path.display().to_string());
        }

        ContextBundle {
            library_summary,
            folder_summaries,
            relevant_files,
            relevant_sections: vec![],
            metadata: vec![],
        }
    }
}
