// Aroxy System & Process Resource Metrics

pub fn get_process_memory_mb() -> f64 {
    #[cfg(target_os = "linux")]
    {
        // Read RssAnon (private heap & stack allocated memory) rather than
        // total VmRSS which includes Mesa's shared GPU driver libraries (libLLVM ~180MB)
        if let Ok(content) = std::fs::read_to_string("/proc/self/status") {
            for line in content.lines() {
                if line.starts_with("RssAnon:") {
                    if let Some(val_str) = line.split_whitespace().nth(1) {
                        if let Ok(kb) = val_str.parse::<f64>() {
                            return kb / 1024.0;
                        }
                    }
                }
            }
        }

        if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
            let parts: Vec<&str> = content.split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(resident_pages) = parts[1].parse::<u64>() {
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
