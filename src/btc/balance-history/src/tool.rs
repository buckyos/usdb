use crate::db::BalanceHistoryDB;
use std::path::Path;
use daemonize::Daemonize;
use std::fs::File;

pub fn clear_db_files(data_dir: &Path) -> Result<(), String> {
    let db_dir = BalanceHistoryDB::get_db_dir(data_dir);
    if db_dir.exists() {
        std::fs::remove_dir_all(&db_dir).map_err(|e| {
            let msg = format!(
                "Could not delete database directory at {}: {}",
                db_dir.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;
        println!("Deleted RocksDB directory at {}", db_dir.display());
    } else {
        println!("RocksDB directory does not exist at {}", db_dir.display());
    }

    Ok(())
}

pub fn daemonize_process(service_name: &str) {
    let root_dir = usdb_util::get_service_dir(service_name);
    if !root_dir.exists() {
        std::fs::create_dir_all(&root_dir).expect("Failed to create service root directory");
    }

    let pid_file = root_dir.join(format!("{}.pid", service_name));
    let daemonize = Daemonize::new()
        .pid_file(pid_file) // Specify pid file
        .chown_pid_file(true) // Change ownership of pid file
        .stdout(File::open("/dev/null").unwrap())
        .stderr(File::open("/dev/null").unwrap())
        .working_directory(root_dir); // Set working directory;

    match daemonize.start() {
        Ok(_) => {
            info!("{} service daemonized successfully", service_name);
        }
        Err(e) => {
            println!("Error daemonizing {} service: {}", service_name, e);
            std::process::exit(1);
        }
    }
}