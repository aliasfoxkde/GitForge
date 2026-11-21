//! Host resource pressure admission for storage-heavy CI jobs.
//!
//! Linux PSI is measured on the scheduler host and applies only to runners
//! declaring that same storage domain. Remote runner domains are not inferred
//! from this host's counters.

#[cfg(not(test))]
use std::fs;
#[cfg(not(test))]
use std::path::Path;

pub const DEFAULT_IO_FULL_AVG10_LIMIT_PCT: f64 = 25.0;
pub const DEFAULT_CPU_SOME_AVG10_LIMIT_PCT: f64 = 80.0;
pub const IO_FULL_AVG10_LIMIT_ENV: &str = "GITFORGE_IO_FULL_AVG10_LIMIT_PCT";
pub const STORAGE_DOMAIN_ENV: &str = "GITFORGE_STORAGE_DOMAIN";
#[cfg(not(test))]
const LINUX_IO_PRESSURE_PATH: &str = "/proc/pressure/io";
#[cfg(not(test))]
const LINUX_CPU_PRESSURE_PATH: &str = "/proc/pressure/cpu";
#[cfg(not(test))]
const PROC_ROOT: &str = "/proc";
const ACTIVE_BUILD_PROCESS_NAMES: &[&str] = &[
    "cargo",
    "cargo-clippy",
    "rustc",
    "clippy-driver",
    "rust-lld",
    "ld.lld",
    // These build drivers and postprocessors can outlive Cargo or remain
    // active after its wrapper exits. The local host manager treats them as
    // build pressure, so the scheduler must do the same before dispatching
    // another storage-sensitive job.
    "trunk",
    "wasm-bindgen",
    "wasm-opt",
    "xtask",
];

fn is_active_build_process_name(comm: &str) -> bool {
    ACTIVE_BUILD_PROCESS_NAMES.contains(&comm.trim())
}

#[derive(Clone, Debug, PartialEq)]
pub struct HostIoPressure {
    full_avg10_pct: Option<f64>,
    cpu_some_avg10_pct: Option<f64>,
    limit_pct: f64,
    storage_domain: String,
    active_build_pids: Option<Vec<u32>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostIoDeferral {
    CpuPressure,
    CpuPressureUnavailable,
    Pressure,
    PressureUnavailable,
    ActiveBuild,
    ProcessProbeUnavailable,
}

impl HostIoPressure {
    pub fn read_system() -> Self {
        let limit_pct = configured_limit_pct();
        // Keep the scheduler unit suite independent of the machine's current
        // load. Pressure-specific cases pass explicit PSI fixtures below.
        #[cfg(test)]
        {
            Self::from_text(
                "some avg10=0 avg60=0 avg300=0 total=0\nfull avg10=0 avg60=0 avg300=0 total=0\n",
                limit_pct,
            )
            .with_active_build_pids(Some(Vec::new()))
        }
        #[cfg(not(test))]
        {
            let storage_domain = configured_storage_domain();
            let mut pressure = match fs::read_to_string(LINUX_IO_PRESSURE_PATH) {
                Ok(contents) => {
                    let mut pressure = Self::from_text(&contents, limit_pct);
                    pressure.storage_domain = storage_domain.clone();
                    pressure
                }
                Err(_) => Self {
                    full_avg10_pct: None,
                    cpu_some_avg10_pct: None,
                    limit_pct,
                    storage_domain,
                    active_build_pids: None,
                },
            };
            pressure.active_build_pids = read_active_build_pids(Path::new(PROC_ROOT));
            pressure.cpu_some_avg10_pct = fs::read_to_string(LINUX_CPU_PRESSURE_PATH)
                .ok()
                .and_then(|contents| parse_avg10_pct(&contents, "some"));
            pressure
        }
    }

    pub fn from_text(contents: &str, limit_pct: f64) -> Self {
        Self {
            full_avg10_pct: parse_full_avg10_pct(contents),
            // Existing I/O-only fixtures model a healthy CPU host. CPU-specific
            // cases opt into an explicit measurement below.
            cpu_some_avg10_pct: Some(0.0),
            limit_pct: normalize_limit_pct(limit_pct),
            storage_domain: "test-domain".to_string(),
            active_build_pids: Some(Vec::new()),
        }
    }

    #[cfg(test)]
    pub fn with_active_build_pids(mut self, pids: Option<Vec<u32>>) -> Self {
        self.active_build_pids = pids;
        self
    }

    #[cfg(test)]
    pub fn with_cpu_some_avg10_pct(mut self, pressure: Option<f64>) -> Self {
        self.cpu_some_avg10_pct = pressure;
        self
    }

    pub fn full_avg10_pct(&self) -> Option<f64> {
        self.full_avg10_pct
    }

    pub fn cpu_some_avg10_pct(&self) -> Option<f64> {
        self.cpu_some_avg10_pct
    }

    pub fn limit_pct(&self) -> f64 {
        self.limit_pct
    }

    pub fn storage_domain(&self) -> &str {
        &self.storage_domain
    }

    pub fn deferral(&self) -> Option<HostIoDeferral> {
        match self.cpu_some_avg10_pct {
            None => return Some(HostIoDeferral::CpuPressureUnavailable),
            Some(pressure) if pressure > DEFAULT_CPU_SOME_AVG10_LIMIT_PCT => {
                return Some(HostIoDeferral::CpuPressure);
            }
            Some(_) => {}
        }

        match self.full_avg10_pct {
            None => return Some(HostIoDeferral::PressureUnavailable),
            Some(pressure) if pressure > self.limit_pct => {
                return Some(HostIoDeferral::Pressure);
            }
            Some(_) => {}
        }

        match self.active_build_pids.as_deref() {
            None => Some(HostIoDeferral::ProcessProbeUnavailable),
            Some(pids) if !pids.is_empty() => Some(HostIoDeferral::ActiveBuild),
            Some(_) => None,
        }
    }

    pub fn unavailable(&self) -> bool {
        self.full_avg10_pct.is_none()
            || self.cpu_some_avg10_pct.is_none()
            || self.active_build_pids.is_none()
    }

    pub fn active_build_count(&self) -> Option<usize> {
        self.active_build_pids.as_ref().map(Vec::len)
    }
}

#[cfg(not(test))]
fn read_active_build_pids(proc_root: &Path) -> Option<Vec<u32>> {
    let entries = fs::read_dir(proc_root).ok()?;
    let mut pids = Vec::new();
    for entry in entries {
        let entry = entry.ok()?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let process_dir = entry.path();
        let comm = match fs::read_to_string(process_dir.join("comm")) {
            Ok(comm) => comm,
            // A process can exit between read_dir and reading comm. Other
            // failures mean the scan cannot prove that no compiler is active.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        if !is_active_build_process_name(&comm) {
            continue;
        }
        let state = match fs::read_to_string(process_dir.join("status")) {
            Ok(status) => status.lines().find_map(|line| {
                line.strip_prefix("State:")
                    .and_then(|state| state.split_whitespace().next())
                    .map(str::to_string)
            }),
            // Treat permission or parse failures conservatively as active.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => None,
        };
        if state.as_deref() != Some("Z") {
            pids.push(pid);
        }
    }
    Some(pids)
}

fn configured_limit_pct() -> f64 {
    std::env::var(IO_FULL_AVG10_LIMIT_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .map_or(DEFAULT_IO_FULL_AVG10_LIMIT_PCT, normalize_limit_pct)
}

#[cfg(not(test))]
fn configured_storage_domain() -> String {
    std::env::var(STORAGE_DOMAIN_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            fs::read_to_string("/etc/hostname")
                .ok()
                .map(|hostname| hostname.trim().to_string())
                .filter(|hostname| !hostname.is_empty())
        })
        .unwrap_or_else(|| "default".to_string())
}

fn normalize_limit_pct(limit_pct: f64) -> f64 {
    if limit_pct.is_finite() {
        limit_pct.clamp(0.0, 100.0)
    } else {
        DEFAULT_IO_FULL_AVG10_LIMIT_PCT
    }
}

fn parse_full_avg10_pct(contents: &str) -> Option<f64> {
    parse_avg10_pct(contents, "full")
}

fn parse_avg10_pct(contents: &str, kind: &str) -> Option<f64> {
    let prefix = format!("{kind} ");
    let line = contents
        .lines()
        .find(|line| line.starts_with(prefix.as_str()))?;
    let raw = line
        .split_whitespace()
        .find_map(|field| field.strip_prefix("avg10="))?;
    let value = raw.parse::<f64>().ok()?;
    (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "some avg10=91.10 avg60=80.00 avg300=70.00 total=1\nfull avg10=24.90 avg60=20.00 avg300=10.00 total=2\n";

    #[test]
    fn recognizes_build_drivers_and_postprocessors() {
        for process in ["xtask", "trunk", "wasm-bindgen", "wasm-opt"] {
            assert!(is_active_build_process_name(process), "{process}");
        }
        assert!(!is_active_build_process_name("bash"));
    }

    #[test]
    fn parses_full_pressure_instead_of_some_pressure() {
        let pressure = HostIoPressure::from_text(SAMPLE, 25.0);
        assert_eq!(pressure.full_avg10_pct(), Some(24.9));
        assert_eq!(pressure.deferral(), None);
    }

    #[test]
    fn parses_cpu_some_pressure_avg10() {
        assert_eq!(
            parse_avg10_pct("some avg10=80.1 avg60=70 avg300=60 total=1\n", "some"),
            Some(80.1)
        );
        assert_eq!(
            parse_avg10_pct("some avg10=bad avg60=70 avg300=60 total=1\n", "some"),
            None
        );
    }

    #[test]
    fn blocks_heavy_work_above_the_shared_limit() {
        let pressure = HostIoPressure::from_text(
            "some avg10=99.0 avg60=90.0 avg300=80.0 total=1\nfull avg10=25.1 avg60=25.0 avg300=24.0 total=2\n",
            25.0,
        );
        assert_eq!(pressure.deferral(), Some(HostIoDeferral::Pressure));
        assert!(!pressure.unavailable());
    }

    #[test]
    fn blocks_storage_work_when_cpu_pressure_exceeds_host_gate() {
        let pressure = HostIoPressure::from_text(SAMPLE, 25.0)
            .with_cpu_some_avg10_pct(Some(80.1))
            .with_active_build_pids(Some(Vec::new()));
        assert_eq!(pressure.deferral(), Some(HostIoDeferral::CpuPressure));
    }

    #[test]
    fn missing_cpu_pressure_fails_closed() {
        let pressure = HostIoPressure::from_text(SAMPLE, 25.0)
            .with_cpu_some_avg10_pct(None)
            .with_active_build_pids(Some(Vec::new()));
        assert_eq!(
            pressure.deferral(),
            Some(HostIoDeferral::CpuPressureUnavailable)
        );
        assert!(pressure.unavailable());
    }

    #[test]
    fn allows_work_at_the_limit() {
        let pressure = HostIoPressure::from_text(
            "some avg10=30 avg60=20 avg300=10 total=1\nfull avg10=25.0 avg60=20 avg300=10 total=2\n",
            25.0,
        );
        assert_eq!(pressure.deferral(), None);
    }

    #[test]
    fn malformed_or_missing_full_pressure_fails_closed() {
        for input in ["some avg10=0.0\n", "full avg10=bad\n", "full avg10=101\n"] {
            let pressure = HostIoPressure::from_text(input, 25.0);
            assert!(pressure.unavailable(), "input: {input:?}");
            assert_eq!(
                pressure.deferral(),
                Some(HostIoDeferral::PressureUnavailable),
                "input: {input:?}"
            );
        }
    }

    #[test]
    fn active_host_compilers_defer_even_when_psi_is_below_the_limit() {
        let pressure = HostIoPressure::from_text(
            "some avg10=10 avg60=10 avg300=10 total=1\nfull avg10=1.0 avg60=1 avg300=1 total=2\n",
            25.0,
        )
        .with_active_build_pids(Some(vec![1234]));
        assert_eq!(pressure.deferral(), Some(HostIoDeferral::ActiveBuild));
        assert_eq!(pressure.active_build_count(), Some(1));
    }

    #[test]
    fn missing_process_probe_fails_closed_even_when_psi_is_low() {
        let pressure = HostIoPressure::from_text(SAMPLE, 25.0).with_active_build_pids(None);
        assert_eq!(
            pressure.deferral(),
            Some(HostIoDeferral::ProcessProbeUnavailable)
        );
        assert!(pressure.unavailable());
    }

    #[test]
    fn normalizes_configured_limits_to_percentage_range() {
        assert_eq!(HostIoPressure::from_text(SAMPLE, -1.0).limit_pct(), 0.0);
        assert_eq!(HostIoPressure::from_text(SAMPLE, 101.0).limit_pct(), 100.0);
        assert_eq!(
            HostIoPressure::from_text(SAMPLE, f64::NAN).limit_pct(),
            DEFAULT_IO_FULL_AVG10_LIMIT_PCT
        );
    }
}
