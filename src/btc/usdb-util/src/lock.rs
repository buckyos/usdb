
use named_lock::{NamedLock, NamedLockGuard};
use std::process::exit;
use super::dirs::get_service_dir;

pub fn init_process_lock(service_name: &str) -> (NamedLock, NamedLockGuard) {
    let dir = get_service_dir(service_name);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| {
        println!("Failed to create service directory {:?}: {}", dir, e);
        exit(1);
    });

    // Acquire application lock to prevent multiple instances\
    let lock_name = format!("{}_lock", service_name);
    let lock = match NamedLock::create(&lock_name){
        Ok(lock) => lock,
        Err(e) => {
            println!("Failed to acquire application lock: {}", e);
            exit(1);
        }
    };

    let guard = match lock.lock() {
        Ok(_lock_guard) => { _lock_guard },
        Err(e) => {
            println!("Another instance is already running: {}", e);
            exit(1);
        }
    };

    (lock, guard)
}