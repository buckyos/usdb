use is_docker::is_docker;

pub fn get_smart_memory_limit() -> u64 {
    // First check if we are running inside a container
    if is_docker() {
        info!("Running inside a container, checking for memory limits");
        if let Some(limit) = get_container_limit() {
            info!("Container memory limit detected: {} bytes", limit);
            return limit;
        }

        warn!("No container memory limit detected, falling back to physical memory");
    }

    // 2. If not in a container, or the container has no limit set, return the total physical memory
    let mut sys = sysinfo::System::new_all();
    sys.refresh_memory();
    
    let bytes = sys.total_memory(); // TODO: consider using total_memory vs available_memory?
    info!("Physical total memory: {} bytes", bytes);

    bytes
}

fn get_container_limit() -> Option<u64> {
    // Try Cgroup v2 (modern container environments)
    if let Ok(content) = std::fs::read_to_string("/sys/fs/cgroup/memory.max") {
        let val = content.trim();
        if val != "max" {
            return val.parse().ok();
        }
    }

    // Try Cgroup v1
    if let Ok(content) = std::fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes") {
        return content.trim().parse().ok();
    }

    None
}