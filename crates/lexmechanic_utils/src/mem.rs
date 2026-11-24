use memory_stats::memory_stats;

pub fn memory(since: f64) -> f64 {
    if let Some(usage) = memory_stats() {
        let mb = usage.physical_mem as f64 / (1024.0 * 1024.0);
        mb - since
    } else {
        0.0
    }
}
