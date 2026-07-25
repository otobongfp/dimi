use dimi_runtime::kernel::config::BootConfig;
use dimi_runtime::services::storage::SqliteStorageEngine;

fn main() {
    let config = BootConfig::load_or_default().expect("failed to load config");
    let db_path = config.db_path();
    println!("Applying migrations to {}", db_path.display());

    match SqliteStorageEngine::open(&db_path) {
        Ok(_) => println!("Migrations applied successfully."),
        Err(e) => {
            eprintln!("Migration failed: {e}");
            std::process::exit(1);
        }
    }
}
