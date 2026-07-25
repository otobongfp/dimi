use crate::common::ProcessMemory;
use serde::{Deserialize, Serialize};
use std::path::Path;
use sysinfo::{Components, Disks, System};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RamTier {
    Low,
    Mid,
    High,
}

const GB: u64 = 1024 * 1024 * 1024;

pub fn total_ram_bytes() -> u64 {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.total_memory()
}

pub fn ram_tier() -> RamTier {
    ram_tier_for(total_ram_bytes())
}

fn ram_tier_for(total_bytes: u64) -> RamTier {
    if total_bytes <= 6 * GB {
        RamTier::Low
    } else if total_bytes <= 12 * GB {
        RamTier::Mid
    } else {
        RamTier::High
    }
}

pub fn available_ram_bytes() -> u64 {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.available_memory()
}

pub fn physical_core_count() -> i32 {
    sysinfo::System::physical_core_count().unwrap_or(4) as i32
}

/// Hottest sensor reading available on this machine, in Celsius. `None` on
/// hardware/sandboxes that expose no thermal sensors (common in CI and some
/// containers) rather than a misleading default — callers must treat that as
/// "no thermal signal," not "cold."
pub fn current_cpu_temp_celsius() -> Option<f32> {
    Components::new_with_refreshed_list()
        .list()
        .iter()
        .filter_map(|c| c.temperature())
        .fold(None, |max: Option<f32>, t| Some(max.map_or(t, |m| m.max(t))))
}

// ADTC's own thermal penalty triggers at 85°C. These are set below that so
// context headroom (the thing we control without interrupting a generation
// already in flight) gets trimmed on the way up, before the system is
// already in the penalty band or visibly throttling.
const THERMAL_WATCH_CELSIUS: f32 = 75.0;
const THERMAL_HOT_CELSIUS: f32 = 82.0;
const THERMAL_MIN_SCALE: f64 = 0.5;

/// How much of the normal context-window RAM budget to actually use, given
/// the current thermal reading. 1.0 below the watch line, tapering linearly
/// down to `THERMAL_MIN_SCALE` at/above the hot line. A smaller context
/// means a smaller KV cache and less sustained compute per turn — the one
/// lever available to reduce heat output without refusing to answer.
pub fn thermal_context_scale(cpu_temp_celsius: Option<f32>) -> f64 {
    let Some(t) = cpu_temp_celsius else {
        return 1.0;
    };
    if t <= THERMAL_WATCH_CELSIUS {
        1.0
    } else if t >= THERMAL_HOT_CELSIUS {
        THERMAL_MIN_SCALE
    } else {
        let span = (THERMAL_HOT_CELSIUS - THERMAL_WATCH_CELSIUS) as f64;
        let over = (t - THERMAL_WATCH_CELSIUS) as f64;
        1.0 - (1.0 - THERMAL_MIN_SCALE) * (over / span)
    }
}

pub fn top_memory_consumers(limit: usize) -> Vec<ProcessMemory> {
    let mut sys = System::new_all();
    sys.refresh_all();
    let own_pid = sysinfo::get_current_pid().ok();

    let mut processes: Vec<ProcessMemory> = sys
        .processes()
        .iter()
        .filter(|(pid, _)| Some(**pid) != own_pid)
        .map(|(pid, process)| ProcessMemory {
            pid: pid.as_u32(),
            name: process.name().to_string_lossy().into_owned(),
            ram_bytes: process.memory(),
        })
        .collect();
    processes.sort_by(|a, b| b.ram_bytes.cmp(&a.ram_bytes));
    processes.truncate(limit);
    processes
}

const CONTEXT_RAM_SAFETY_MARGIN_BYTES: u64 = GB;

const CONTEXT_RAM_FRACTION: f64 = 0.5;

pub const MIN_CONTEXT_TOKENS: u32 = 1024;
pub const MAX_CONTEXT_TOKENS: u32 = 8192;

pub fn parse_memory_budget(setting: &str) -> Option<u64> {
    match setting {
        "2GB" => Some(2 * GB),
        "3GB" => Some(3 * GB),
        "4GB" => Some(4 * GB),
        "6GB" => Some(6 * GB),
        "8GB" => Some(8 * GB),
        "12GB" => Some(12 * GB),
        _ => None,
    }
}

/// `thermal_scale` trims the RAM budget this function would otherwise use
/// (1.0 = no change; see `thermal_context_scale`) — it never pushes the
/// result below `MIN_CONTEXT_TOKENS`, and a caller's own `needed` floor
/// (prompt + max generation length) still wins via `.max()` at the call
/// site, so this only ever gives back unused headroom, never breaks a turn.
pub fn dynamic_context_tokens(
    kv_bytes_per_token: u64,
    memory_budget_bytes: Option<u64>,
    model_size_bytes: u64,
    thermal_scale: f64,
) -> u32 {
    let budget_bytes = if let Some(budget) = memory_budget_bytes {
        budget.saturating_sub(model_size_bytes)
    } else {
        let available = available_ram_bytes();
        let usable = available.saturating_sub(CONTEXT_RAM_SAFETY_MARGIN_BYTES);
        (usable as f64 * CONTEXT_RAM_FRACTION) as u64
    };
    let budget_bytes = (budget_bytes as f64 * thermal_scale.clamp(0.0, 1.0)) as u64;

    let tokens = budget_bytes / kv_bytes_per_token.max(1);
    u32::try_from(tokens)
        .unwrap_or(u32::MAX)
        .clamp(MIN_CONTEXT_TOKENS, MAX_CONTEXT_TOKENS)
}

pub fn required_ram_bytes(model_size_bytes: u64, kv_bytes_per_token: u64) -> u64 {
    model_size_bytes
        + (MIN_CONTEXT_TOKENS as u64 * kv_bytes_per_token)
        + CONTEXT_RAM_SAFETY_MARGIN_BYTES
}

const PREFLIGHT_TOP_CONSUMERS: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcePreflight {
    pub available_bytes: u64,
    pub required_bytes: u64,
    pub sufficient: bool,
    pub top_consumers: Vec<ProcessMemory>,
}

pub fn resource_preflight(model_size_bytes: u64, kv_bytes_per_token: u64) -> ResourcePreflight {
    let available_bytes = available_ram_bytes();
    let required_bytes = required_ram_bytes(model_size_bytes, kv_bytes_per_token);
    let sufficient = available_bytes >= required_bytes;
    let top_consumers = if sufficient {
        Vec::new()
    } else {
        top_memory_consumers(PREFLIGHT_TOP_CONSUMERS)
    };
    ResourcePreflight {
        available_bytes,
        required_bytes,
        sufficient,
        top_consumers,
    }
}

pub fn available_disk_bytes(path: &Path) -> u64 {
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        match probe.parent() {
            Some(parent) => probe = parent.to_path_buf(),
            None => break,
        }
    }
    let probe = probe.canonicalize().unwrap_or(probe);

    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| probe.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_bucket_by_total_ram() {
        assert_eq!(ram_tier_for(4 * GB), RamTier::Low);
        assert_eq!(ram_tier_for(6 * GB), RamTier::Low);
        assert_eq!(ram_tier_for(8 * GB), RamTier::Mid);
        assert_eq!(ram_tier_for(12 * GB), RamTier::Mid);
        assert_eq!(ram_tier_for(16 * GB), RamTier::High);
    }

    #[test]
    fn available_disk_bytes_is_nonzero_for_a_real_path() {
        let bytes = available_disk_bytes(&std::env::temp_dir());
        assert!(
            bytes > 0,
            "expected the temp dir's filesystem to report some free space"
        );
    }

    #[test]
    fn dynamic_context_tokens_stays_within_bounds() {
        let tokens = dynamic_context_tokens(147_456, None, 0, 1.0);
        assert!(tokens >= MIN_CONTEXT_TOKENS && tokens <= MAX_CONTEXT_TOKENS);
    }

    #[test]
    fn required_ram_bytes_includes_model_kv_and_safety_margin() {
        let required = required_ram_bytes(1 * GB, 100_000);
        assert!(required > 1 * GB, "should add KV cache + safety margin on top of model size");
    }

    #[test]
    fn resource_preflight_flags_a_giant_model_insufficient() {
        let preflight = resource_preflight(10_000 * GB, 147_456);
        assert!(!preflight.sufficient);
        assert!(!preflight.top_consumers.is_empty());
    }

    #[test]
    fn resource_preflight_skips_process_scan_when_sufficient() {
        let preflight = resource_preflight(1024, 1);
        assert!(preflight.sufficient);
        assert!(preflight.top_consumers.is_empty());
    }

    #[test]
    fn dynamic_context_tokens_shrinks_as_kv_cost_grows() {
        let cheap = dynamic_context_tokens(50_000, None, 0, 1.0);
        let expensive = dynamic_context_tokens(5_000_000, None, 0, 1.0);
        assert!(cheap >= expensive);
    }

    #[test]
    fn dynamic_context_tokens_shrinks_under_thermal_scale() {
        // Chosen so the unclamped budget/kv result lands inside
        // [MIN_CONTEXT_TOKENS, MAX_CONTEXT_TOKENS] — otherwise the clamp
        // masks any difference the thermal scale would otherwise produce.
        let budget = 4 * GB;
        let kv_bytes_per_token = 1_000_000;
        let cool = dynamic_context_tokens(kv_bytes_per_token, Some(budget), 0, 1.0);
        let hot = dynamic_context_tokens(kv_bytes_per_token, Some(budget), 0, 0.5);
        assert!(hot < cool, "a lower thermal scale should reduce the context budget");
        assert!(hot >= MIN_CONTEXT_TOKENS, "must never scale below the hard floor");
    }

    #[test]
    fn thermal_context_scale_is_full_below_the_watch_line() {
        assert_eq!(thermal_context_scale(Some(60.0)), 1.0);
        assert_eq!(thermal_context_scale(None), 1.0);
    }

    #[test]
    fn thermal_context_scale_tapers_between_watch_and_hot() {
        let mid = thermal_context_scale(Some((THERMAL_WATCH_CELSIUS + THERMAL_HOT_CELSIUS) / 2.0));
        assert!(mid > THERMAL_MIN_SCALE && mid < 1.0);
    }

    #[test]
    fn thermal_context_scale_floors_at_the_hot_line() {
        assert_eq!(thermal_context_scale(Some(THERMAL_HOT_CELSIUS)), THERMAL_MIN_SCALE);
        assert_eq!(thermal_context_scale(Some(95.0)), THERMAL_MIN_SCALE);
    }
}
