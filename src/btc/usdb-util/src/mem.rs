use std::fmt;
use std::path::{Path, PathBuf};

const CGROUP_ROOT: &str = "/sys/fs/cgroup";
const PROC_SELF_CGROUP: &str = "/proc/self/cgroup";
const CGROUP_UNLIMITED_THRESHOLD: u64 = 1 << 60;

/// Source used to account for the process memory budget and current usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryAccountingSource {
    /// Unified cgroup hierarchy using memory.max and memory.current.
    CgroupV2,
    /// Legacy memory controller using memory.limit_in_bytes and memory.usage_in_bytes.
    CgroupV1,
    /// Host physical memory reported by sysinfo when no finite cgroup limit applies.
    Physical,
}

impl fmt::Display for MemoryAccountingSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CgroupV2 => formatter.write_str("cgroup-v2"),
            Self::CgroupV1 => formatter.write_str("cgroup-v1"),
            Self::Physical => formatter.write_str("physical"),
        }
    }
}

/// Point-in-time memory accounting data for the current process environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryUsageSnapshot {
    /// Accounting source selected for this snapshot.
    pub source: MemoryAccountingSource,
    /// Effective memory ceiling in bytes.
    pub limit_bytes: u64,
    /// Current usage in bytes within the same accounting scope as the limit.
    pub used_bytes: u64,
}

impl MemoryUsageSnapshot {
    /// Returns usage as an integer percentage of the effective memory limit.
    pub fn used_percent(&self) -> u64 {
        if self.limit_bytes == 0 {
            return 0;
        }
        ((self.used_bytes as u128 * 100) / self.limit_bytes as u128) as u64
    }
}

/// Returns the cgroup-aware memory limit available to this process.
pub fn get_smart_memory_limit() -> u64 {
    get_memory_usage_snapshot().limit_bytes
}

/// Returns current cgroup v2/v1 usage and limit, falling back to host memory.
pub fn get_memory_usage_snapshot() -> MemoryUsageSnapshot {
    let physical = physical_memory_snapshot();
    let proc_cgroup = std::fs::read_to_string(PROC_SELF_CGROUP).unwrap_or_default();

    detect_cgroup_memory(Path::new(CGROUP_ROOT), &proc_cgroup, physical.limit_bytes)
        .unwrap_or(physical)
}

fn physical_memory_snapshot() -> MemoryUsageSnapshot {
    let mut system = sysinfo::System::new_all();
    system.refresh_memory();
    MemoryUsageSnapshot {
        source: MemoryAccountingSource::Physical,
        limit_bytes: system.total_memory(),
        used_bytes: system.used_memory(),
    }
}

fn detect_cgroup_memory(
    cgroup_root: &Path,
    proc_cgroup: &str,
    physical_limit: u64,
) -> Option<MemoryUsageSnapshot> {
    let v2_path = parse_cgroup_path(proc_cgroup, None).unwrap_or("/");
    if let Some(snapshot) = read_cgroup_hierarchy(
        cgroup_root,
        v2_path,
        "memory.max",
        "memory.current",
        MemoryAccountingSource::CgroupV2,
        physical_limit,
    ) {
        return Some(snapshot);
    }

    let v1_root = cgroup_root.join("memory");
    let v1_path = parse_cgroup_path(proc_cgroup, Some("memory")).unwrap_or("/");
    read_cgroup_hierarchy(
        &v1_root,
        v1_path,
        "memory.limit_in_bytes",
        "memory.usage_in_bytes",
        MemoryAccountingSource::CgroupV1,
        physical_limit,
    )
}

fn parse_cgroup_path<'a>(proc_cgroup: &'a str, controller: Option<&str>) -> Option<&'a str> {
    proc_cgroup.lines().find_map(|line| {
        let mut fields = line.splitn(3, ':');
        let hierarchy = fields.next()?;
        let controllers = fields.next()?;
        let path = fields.next()?;

        match controller {
            None if hierarchy == "0" && controllers.is_empty() => Some(path),
            Some(expected)
                if controllers
                    .split(',')
                    .any(|controller| controller == expected) =>
            {
                Some(path)
            }
            _ => None,
        }
    })
}

fn read_cgroup_hierarchy(
    root: &Path,
    cgroup_path: &str,
    limit_file: &str,
    usage_file: &str,
    source: MemoryAccountingSource,
    physical_limit: u64,
) -> Option<MemoryUsageSnapshot> {
    cgroup_candidate_dirs(root, cgroup_path)
        .into_iter()
        .filter_map(|dir| {
            let limit = read_memory_value(&dir.join(limit_file))?;
            let used = read_memory_value(&dir.join(usage_file))?;
            if !is_effective_cgroup_limit(limit, physical_limit) {
                return None;
            }
            Some(MemoryUsageSnapshot {
                source,
                limit_bytes: limit,
                used_bytes: used,
            })
        })
        .min_by_key(|snapshot| snapshot.limit_bytes)
}

fn cgroup_candidate_dirs(root: &Path, cgroup_path: &str) -> Vec<PathBuf> {
    let relative = cgroup_path.trim_start_matches('/');
    let mut current = if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    if !current.starts_with(root) {
        return vec![root.to_path_buf()];
    }

    let mut dirs = Vec::new();
    loop {
        dirs.push(current.clone());
        if current == root || !current.pop() || !current.starts_with(root) {
            break;
        }
    }
    if dirs.last().is_none_or(|dir| dir != root) {
        dirs.push(root.to_path_buf());
    }
    dirs
}

fn read_memory_value(path: &Path) -> Option<u64> {
    let value = std::fs::read_to_string(path).ok()?;
    value.trim().parse().ok()
}

fn is_effective_cgroup_limit(limit: u64, physical_limit: u64) -> bool {
    limit > 0
        && limit < CGROUP_UNLIMITED_THRESHOLD
        && (physical_limit == 0 || limit <= physical_limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn temp_root(tag: &str) -> PathBuf {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "usdb_cgroup_memory_{}_{}_{}",
            tag,
            std::process::id(),
            id
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_memory_pair(dir: &Path, limit_name: &str, usage_name: &str, limit: &str, used: u64) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(limit_name), limit).unwrap();
        std::fs::write(dir.join(usage_name), used.to_string()).unwrap();
    }

    #[test]
    fn detects_nested_cgroup_v2_limit() {
        let root = temp_root("v2");
        let current = root.join("system.slice/usdb.service");
        write_memory_pair(&current, "memory.max", "memory.current", "25769803776", 12);

        let snapshot = detect_cgroup_memory(
            &root,
            "0::/system.slice/usdb.service\n",
            64 * 1024 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(snapshot.source, MemoryAccountingSource::CgroupV2);
        assert_eq!(snapshot.limit_bytes, 24 * 1024 * 1024 * 1024);
        assert_eq!(snapshot.used_bytes, 12);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uses_finite_cgroup_v2_ancestor_limit() {
        let root = temp_root("v2_ancestor");
        let parent = root.join("user.slice");
        let current = parent.join("session.scope");
        write_memory_pair(&parent, "memory.max", "memory.current", "1024", 512);
        write_memory_pair(&current, "memory.max", "memory.current", "max", 256);

        let snapshot = detect_cgroup_memory(&root, "0::/user.slice/session.scope\n", 4096).unwrap();
        assert_eq!(snapshot.limit_bytes, 1024);
        assert_eq!(snapshot.used_bytes, 512);
        assert_eq!(snapshot.used_percent(), 50);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_cgroup_v1_memory_controller() {
        let root = temp_root("v1");
        let current = root.join("memory/docker/test");
        write_memory_pair(
            &current,
            "memory.limit_in_bytes",
            "memory.usage_in_bytes",
            "2048",
            1024,
        );

        let snapshot = detect_cgroup_memory(
            &root,
            "5:cpu:/docker/test\n7:memory,blkio:/docker/test\n",
            4096,
        )
        .unwrap();
        assert_eq!(snapshot.source, MemoryAccountingSource::CgroupV1);
        assert_eq!(snapshot.limit_bytes, 2048);
        assert_eq!(snapshot.used_percent(), 50);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignores_unlimited_or_larger_than_physical_cgroup_limits() {
        let root = temp_root("unlimited");
        write_memory_pair(
            &root,
            "memory.max",
            "memory.current",
            &CGROUP_UNLIMITED_THRESHOLD.to_string(),
            1024,
        );
        assert!(detect_cgroup_memory(&root, "0::/\n", 4096).is_none());

        std::fs::write(root.join("memory.max"), "8192").unwrap();
        assert!(detect_cgroup_memory(&root, "0::/\n", 4096).is_none());

        std::fs::remove_dir_all(root).unwrap();
    }
}
