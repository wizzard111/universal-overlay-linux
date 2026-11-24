//! Linux sensors from /proc and /sys — no root, no extra packages.
//!
//! * CPU load: /proc/stat deltas; clock: average cpufreq scaling_cur_freq;
//!   temperature: hwmon k10temp (Tctl/Tdie), zenpower, coretemp (Package).
//! * RAM: /proc/meminfo (MemTotal − MemAvailable).
//! * AMD GPU (amdgpu): gpu_busy_percent, hwmon edge/junction/mem temps,
//!   power, sclk/mclk, fan, voltage, mem_info_vram_used.
//! * Intel GPU (i915/xe): current GT frequency.
//! * NVIDIA: NVML.

use super::{nvml, GpuSource, Snapshot};
use std::path::{Path, PathBuf};
use uo_common::sensor::SensorId;

fn read_str(p: &Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
}

fn read_f64(p: &Path) -> Option<f64> {
    read_str(p)?.parse().ok()
}

fn hwmon_dirs() -> Vec<PathBuf> {
    std::fs::read_dir("/sys/class/hwmon")
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// hwmon `tempN_input` whose `tempN_label` matches one of `labels`
/// (or `temp1_input` when `labels` is empty).
fn hwmon_temp(dir: &Path, labels: &[&str]) -> Option<PathBuf> {
    if labels.is_empty() {
        let p = dir.join("temp1_input");
        return p.is_file().then_some(p);
    }
    for i in 1..=16 {
        if let Some(l) = read_str(&dir.join(format!("temp{i}_label"))) {
            if labels.iter().any(|x| l.eq_ignore_ascii_case(x)) {
                return Some(dir.join(format!("temp{i}_input")));
            }
        }
    }
    None
}

// ---------------------------------------------------------------- CPU / RAM

struct Cpu {
    last: Option<(u64, u64)>,
    freq_files: Vec<PathBuf>,
    temp: Option<PathBuf>,
}

impl Cpu {
    fn new() -> Self {
        let mut freq_files: Vec<PathBuf> = std::fs::read_dir("/sys/devices/system/cpu")
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                            n.strip_prefix("cpu").is_some_and(|d| {
                                !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())
                            })
                        })
                    })
                    .map(|p| p.join("cpufreq/scaling_cur_freq"))
                    .filter(|p| p.is_file())
                    .collect()
            })
            .unwrap_or_default();
        freq_files.sort();
        let mut temp = None;
        for d in hwmon_dirs() {
            match read_str(&d.join("name")).as_deref() {
                Some("k10temp") | Some("zenpower") => {
                    temp = hwmon_temp(&d, &["Tdie", "Tctl"]).or_else(|| hwmon_temp(&d, &[]));
                }
                Some("coretemp") => {
                    temp = hwmon_temp(&d, &["Package id 0"]).or_else(|| hwmon_temp(&d, &[]))
                }
                Some("cpu_thermal") if temp.is_none() => temp = hwmon_temp(&d, &[]),
                _ => {}
            }
            if temp.is_some() {
                break;
            }
        }
        Self {
            last: None,
            freq_files,
            temp,
        }
    }

    fn poll(&mut self, s: &mut Snapshot) {
        // /proc/stat: "cpu  user nice system idle iowait irq softirq steal ..."
        if let Some(line) = std::fs::read_to_string("/proc/stat")
            .ok()
            .and_then(|t| t.lines().next().map(str::to_string))
        {
            let v: Vec<u64> = line
                .split_whitespace()
                .skip(1)
                .filter_map(|x| x.parse().ok())
                .collect();
            if v.len() >= 5 {
                let idle = v[3] + v[4];
                let total: u64 = v.iter().take(8).sum();
                if let Some((pi, pt)) = self.last.replace((idle, total)) {
                    let dt = total.saturating_sub(pt);
                    if dt > 0 {
                        let busy = 1.0 - idle.saturating_sub(pi) as f64 / dt as f64;
                        s.set(SensorId::CpuLoad, Some((busy * 100.0).clamp(0.0, 100.0)));
                    }
                }
            }
        }
        let freqs: Vec<f64> = self.freq_files.iter().filter_map(|p| read_f64(p)).collect();
        if !freqs.is_empty() {
            s.set(
                SensorId::CpuClock,
                Some(freqs.iter().sum::<f64>() / freqs.len() as f64 / 1000.0),
            );
        }
        s.set(
            SensorId::CpuTemp,
            self.temp.as_deref().and_then(read_f64).map(|m| m / 1000.0),
        );

        if let Ok(mi) = std::fs::read_to_string("/proc/meminfo") {
            let field = |k: &str| -> Option<f64> {
                mi.lines()
                    .find(|l| l.starts_with(k))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|v| v.parse().ok())
            };
            if let (Some(total), Some(avail)) = (field("MemTotal:"), field("MemAvailable:")) {
                let used = total - avail; // kB
                s.set(SensorId::RamUsed, Some(used / 1024.0));
                s.set(SensorId::RamLoad, Some(used * 100.0 / total));
            }
        }
    }
}

// ---------------------------------------------------------------- GPU (sysfs)

struct SysfsGpu {
    name: String,
    dev: PathBuf,
    hwmon: Option<PathBuf>,
    amd: bool,
}

impl SysfsGpu {
    /// The DRM card with the most VRAM (= the discrete GPU on hybrid laptops).
    fn find() -> Option<Self> {
        let mut best: Option<(u64, SysfsGpu)> = None;
        let rd = std::fs::read_dir("/sys/class/drm").ok()?;
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if !n.starts_with("card") || n.contains('-') {
                continue;
            }
            let dev = e.path().join("device");
            let vendor = read_str(&dev.join("vendor")).unwrap_or_default();
            let amd = vendor == "0x1002";
            let intel = vendor == "0x8086";
            if !amd && !intel {
                continue; // NVIDIA is handled by NVML
            }
            let vram = read_f64(&dev.join("mem_info_vram_total")).unwrap_or(0.0) as u64;
            let hwmon = std::fs::read_dir(dev.join("hwmon"))
                .ok()
                .and_then(|mut r| r.next())
                .and_then(|e| e.ok())
                .map(|e| e.path());
            let name = format!("{} GPU ({})", if amd { "AMD" } else { "Intel" }, n);
            let g = SysfsGpu {
                name,
                dev,
                hwmon,
                amd,
            };
            if best.as_ref().is_none_or(|(v, _)| vram > *v) {
                best = Some((vram, g));
            }
        }
        best.map(|b| b.1)
    }
}

impl GpuSource for SysfsGpu {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn poll(&mut self, s: &mut Snapshot) {
        let d = &self.dev;
        if self.amd {
            s.set(SensorId::GpuLoad, read_f64(&d.join("gpu_busy_percent")));
            s.set(
                SensorId::GpuVram,
                read_f64(&d.join("mem_info_vram_used")).map(|b| b / (1024.0 * 1024.0)),
            );
        } else {
            let f = read_f64(&d.join("drm").join("gt_cur_freq_mhz"))
                .or_else(|| read_f64(&d.join("tile0/gt0/freq0/cur_freq")));
            s.set(SensorId::GpuClock, f);
        }
        if let Some(h) = &self.hwmon {
            let t = |labels: &[&str]| {
                hwmon_temp(h, labels)
                    .as_deref()
                    .and_then(read_f64)
                    .map(|m| m / 1000.0)
            };
            s.set(SensorId::GpuTemp, t(&["edge"]).or_else(|| t(&[])));
            s.set(SensorId::GpuHotspot, t(&["junction"]));
            s.set(SensorId::GpuMemTemp, t(&["mem"]));
            // Power in microwatts: average (older kernels) or input (newer).
            let p =
                read_f64(&h.join("power1_average")).or_else(|| read_f64(&h.join("power1_input")));
            s.set(SensorId::GpuPower, p.map(|uw| uw / 1e6));
            if self.amd {
                s.set(
                    SensorId::GpuClock,
                    read_f64(&h.join("freq1_input")).map(|hz| hz / 1e6),
                );
                s.set(
                    SensorId::GpuMemClock,
                    read_f64(&h.join("freq2_input")).map(|hz| hz / 1e6),
                );
                s.set(SensorId::GpuVoltage, read_f64(&h.join("in0_input")));
            }
            s.set(SensorId::GpuFan, read_f64(&h.join("fan1_input")));
            let pwm = read_f64(&h.join("pwm1"));
            let max = read_f64(&h.join("pwm1_max")).unwrap_or(255.0);
            s.set(SensorId::GpuFanPct, pwm.map(|v| v * 100.0 / max));
        }
    }
}

// ---------------------------------------------------------------- entry

pub struct Sensors {
    cpu: Cpu,
    gpu: Option<Box<dyn GpuSource>>,
}

impl Sensors {
    pub fn new() -> Self {
        let gpu: Option<Box<dyn GpuSource>> = if let Some(n) = nvml::Nvml::new() {
            Some(Box::new(n))
        } else if let Some(g) = SysfsGpu::find() {
            Some(Box::new(g))
        } else {
            None
        };
        Self {
            cpu: Cpu::new(),
            gpu,
        }
    }

    pub fn gpu_name(&self) -> String {
        self.gpu
            .as_ref()
            .map(|g| g.name())
            .unwrap_or_else(|| "нет".into())
    }

    pub fn poll(&mut self, s: &mut Snapshot, want: super::Want) {
        if want.cpu || want.ram || want.cpu_temp {
            self.cpu.poll(s);
        }
        if want.gpu {
            if let Some(g) = self.gpu.as_mut() {
                g.poll(s);
            }
        }
    }
}

impl Default for Sensors {
    fn default() -> Self {
        Self::new()
    }
}
