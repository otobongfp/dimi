use crate::kernel::container::ServiceContainer;
use std::path::Path;
use tracing::{info, warn};

pub async fn discover_and_load(container: &ServiceContainer, _plugins_dir: &Path) {
    let manager = match container.plugin_manager() {
        Ok(m) => m,
        Err(e) => {
            warn!("plugin manager not registered: {e}");
            return;
        }
    };

    match manager.discover().await {
        Ok(manifests) => {
            for manifest in manifests {
                info!(plugin = %manifest.name, "discovered plugin");
            }
        }
        Err(e) => {
            warn!("plugin discovery failed: {e}");
        }
    }
}
