use super::constants::USDB_ROOT_DIR;

pub fn get_usdb_root_dir() -> std::path::PathBuf {
    if let Some(home_dir) = dirs::home_dir() {
        home_dir.join(USDB_ROOT_DIR)
    } else {
        std::path::PathBuf::from(".").join(USDB_ROOT_DIR)
    }
}

pub fn get_service_dir(service_name: &str) -> std::path::PathBuf {
    let root_dir = get_usdb_root_dir();
    root_dir.join(service_name)
}