// Aroxy System & Process Resource Metrics

pub fn get_process_memory_mb() -> f64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
            let parts: Vec<&str> = content.split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(resident_pages) = parts[1].parse::<u64>() {
                    // Standard Linux page size is 4096 bytes
                    let bytes = resident_pages * 4096;
                    return (bytes as f64) / (1024.0 * 1024.0);
                }
            }
        }
    }

    // Default estimate if /proc is not available
    16.5
}

pub fn get_process_memory_string() -> String {
    let mb = get_process_memory_mb();
    format!("{:.1} MB", mb)
}
