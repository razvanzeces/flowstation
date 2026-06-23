//! Host system health collector — temperatures, voltages, currents, power.
//!
//! Walks several Linux interfaces and collects whatever is available:
//!
//!   * `vcgencmd pmic_read_adc` — Raspberry Pi 4/5 firmware command, the ONLY
//!     source for PMIC rail voltages and currents on a Pi. We exec it because
//!     the rails aren't exposed through sysfs.
//!   * `/sys/class/hwmon/` — kernel-driver-exposed sensors: CPU temperature,
//!     motherboard voltages on x86 (coretemp, nct6779, etc.), NVMe temperature.
//!   * `/sys/class/powercap/intel-rapl:0/energy_uj` — Intel/AMD package energy
//!     counter, gives accurate x86 CPU power draw via a delta over time.
//!   * `/sys/class/power_supply/BAT*/power_now` — laptop / UPS HAT battery
//!     discharge in microwatts.
//!   * `/sys/class/thermal/thermal_zone*/temp` — generic kernel thermal zones
//!     (extra ARM SoC temps on RPi 4, Orange Pi, etc.) when no hwmon temp exists.
//!
//! Works on any Linux host. RPi 5 → full PMIC rail set with total power.
//! RPi 4 → CPU temp + whatever PMIC rails firmware exposes. x86 → RAPL +
//! motherboard sensors. Laptops → battery discharge. Other → just what hwmon
//! shows; total power may be None but temperatures still appear.
//!
//! Runs in its own background thread because both the shell exec and the
//! sysfs reads can stall briefly during driver IO.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::net_telemetry::channel::TelemetrySink;
use crate::net_telemetry::events::{SysSensor, SysSensorKind, TelemetryEvent};

/// Sampling cadence. 2 s is the sweet spot — fast enough to feel live in the
/// topbar badge, slow enough that the `vcgencmd` exec (~10-30 ms) and a couple
/// of dozen sysfs reads don't add measurable system load.
const POLL_INTERVAL: Duration = Duration::from_millis(2000);

/// Spawn the background sampler. Returns immediately. The worker probes
/// each source on every tick and emits a TelemetryEvent::SysHealth with
/// whatever it managed to collect.
pub fn spawn_sys_health(sink: TelemetrySink) {
    thread::spawn(move || {
        // Probe vcgencmd once up-front. If it works we'll use it on every tick;
        // if it doesn't, we silently skip it and rely on hwmon/RAPL/battery.
        let have_vcgencmd = probe_vcgencmd();
        if have_vcgencmd {
            tracing::info!("sys_telemetry: vcgencmd PMIC available — full Pi power data");
        } else {
            tracing::debug!("sys_telemetry: vcgencmd not available (non-Pi or older firmware)");
        }

        // RAPL gives cumulative microjoules. Power = ΔμJ / Δt. We seed with
        // the first reading and start emitting power numbers from the second
        // tick onward.
        let mut last_rapl: Option<(u64, Instant)> = read_rapl_uj().map(|uj| (uj, Instant::now()));

        loop {
            thread::sleep(POLL_INTERVAL);

            let mut sensors: Vec<SysSensor> = Vec::new();
            let mut total_power_w: Option<f32> = None;

            // ── 1. vcgencmd PMIC (Raspberry Pi) ───────────────────────────
            // This is the only way to get rail-level power on a Pi. The
            // command returns a chunk of "name_A current(N)=…A" and
            // "name_V volt(N)=…V" lines. We pair them by rail name and
            // emit voltage, current, and power = V × I for each.
            if have_vcgencmd {
                if let Some(pmic_total) = read_pmic(&mut sensors) {
                    total_power_w = Some(pmic_total);
                }
            }

            // ── 2. hwmon scan (universal kernel sensors) ──────────────────
            scan_hwmon(&mut sensors);

            // ── 3. RAPL (Intel/AMD x86 only) ──────────────────────────────
            // More accurate CPU package power than anything in hwmon. Only
            // overrides `total_power_w` if no PMIC total was found.
            if let Some(new_uj) = read_rapl_uj() {
                let now = Instant::now();
                if let Some((prev_uj, prev_t)) = last_rapl {
                    let dt = now.duration_since(prev_t).as_secs_f64();
                    if dt > 0.05 && new_uj > prev_uj {
                        let watts = ((new_uj - prev_uj) as f64 / 1_000_000.0 / dt) as f32;
                        sensors.push(SysSensor {
                            name: "CPU package (RAPL)".into(),
                            kind: SysSensorKind::Power,
                            value: watts,
                        });
                        if total_power_w.is_none() {
                            total_power_w = Some(watts);
                        }
                    }
                }
                last_rapl = Some((new_uj, now));
            }

            // ── 4. Battery discharge rate (laptops, UPS HATs) ─────────────
            for battery_path in scan_power_supplies() {
                if let Some(uw) = read_int_file(&battery_path.join("power_now")) {
                    let watts = (uw.unsigned_abs() as f32) / 1_000_000.0;
                    if watts > 0.0 {
                        let name = battery_path
                            .file_name()
                            .map(|s| format!("Battery {}", s.to_string_lossy()))
                            .unwrap_or_else(|| "Battery".into());
                        sensors.push(SysSensor {
                            name,
                            kind: SysSensorKind::Power,
                            value: watts,
                        });
                        if total_power_w.is_none() {
                            total_power_w = Some(watts);
                        }
                    }
                }
            }

            // ── 5. NVMe disk temperatures ─────────────────────────────────
            scan_nvme_temps(&mut sensors);

            // ── 6. Generic thermal zones (fallback for ARM SoCs without hwmon) ─
            if !sensors.iter().any(|s| s.kind == SysSensorKind::Temperature) {
                scan_thermal_zones(&mut sensors);
            }

            sink.send(TelemetryEvent::SysHealth { total_power_w, sensors });
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────
// vcgencmd PMIC (Raspberry Pi)
// ─────────────────────────────────────────────────────────────────────────

/// Returns true if `vcgencmd pmic_read_adc` exits cleanly with non-empty output.
fn probe_vcgencmd() -> bool {
    Command::new("vcgencmd")
        .arg("pmic_read_adc")
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Run `vcgencmd pmic_read_adc`, parse the output, push sensors into `out`,
/// and return the total power computed by summing V × I per rail.
///
/// Output format:
///   3V7_WL_SW_A current(0)=0.04879650A
///   3V3_SYS_A current(1)=0.06050766A
///   …
///   3V7_WL_SW_V volt(8)=3.70627200V
///   3V3_SYS_V volt(9)=3.32766400V
///   …
///
/// Names ending in `_A` are currents, `_V` are voltages. The stripped prefix
/// (e.g. "3V7_WL_SW") is the rail key. We emit V, A, and W (= V × I) for each
/// rail and return the sum of W as the system total.
fn read_pmic(out: &mut Vec<SysSensor>) -> Option<f32> {
    let result = Command::new("vcgencmd").arg("pmic_read_adc").output().ok()?;
    if !result.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&result.stdout);

    use std::collections::HashMap;
    let mut currents: HashMap<String, f32> = HashMap::new();
    let mut voltages: HashMap<String, f32> = HashMap::new();

    // Parser is forgiving: any line matching "<NAME>_<A|V> <something>=<value><unit>"
    // contributes. Anything else is ignored.
    for line in text.lines() {
        let Some((name, value)) = parse_pmic_line(line) else {
            continue;
        };
        if let Some(prefix) = name.strip_suffix("_A") {
            currents.insert(prefix.to_string(), value);
        } else if let Some(prefix) = name.strip_suffix("_V") {
            voltages.insert(prefix.to_string(), value);
        }
    }

    // Stable presentation order: most-interesting rails first so the badge
    // and tab both show the SoC power before peripheral rails.
    const RAIL_ORDER: &[&str] = &[
        "VDD_CORE", // ARM cores — biggest single consumer
        "EXT5V",    // external 5V — voltage-only (no current measured)
        "1V8_SYS",
        "3V3_SYS",
        "1V1_SYS", // SoC supplies
        "0V8_SW",
        "0V8_AON", // switched and always-on
        "DDR_VDD2",
        "DDR_VDDQ",  // memory
        "3V7_WL_SW", // WiFi/BT
        "3V3_DAC",
        "3V3_ADC", // audio
        "HDMI",
        "BATT", // peripherals
    ];

    // Build the ordered key list. Anything that appears in voltages or currents
    // but isn't in our priority list goes at the end.
    let mut ordered: Vec<String> = RAIL_ORDER
        .iter()
        .map(|s| s.to_string())
        .filter(|s| voltages.contains_key(s) || currents.contains_key(s))
        .collect();
    let mut extras: Vec<String> = voltages
        .keys()
        .chain(currents.keys())
        .cloned()
        .filter(|k| !RAIL_ORDER.contains(&k.as_str()))
        .collect();
    extras.sort();
    extras.dedup();
    ordered.extend(extras);

    let mut total_w: f32 = 0.0;
    for rail in &ordered {
        let v = voltages.get(rail).copied();
        let a = currents.get(rail).copied();

        if let Some(volts) = v {
            out.push(SysSensor {
                name: rail.clone(),
                kind: SysSensorKind::Voltage,
                value: volts,
            });
        }
        if let Some(amps) = a {
            out.push(SysSensor {
                name: rail.clone(),
                kind: SysSensorKind::Current,
                value: amps,
            });
        }
        if let (Some(volts), Some(amps)) = (v, a) {
            let watts = volts * amps;
            if watts > 0.0001 {
                out.push(SysSensor {
                    name: rail.clone(),
                    kind: SysSensorKind::Power,
                    value: watts,
                });
                total_w += watts;
            }
        }
    }

    if total_w > 0.0 { Some(total_w) } else { None }
}

/// Parse one line of `vcgencmd pmic_read_adc` output.
/// Returns (rail_name_with_suffix, value) e.g. ("3V3_SYS_A", 0.06050766).
fn parse_pmic_line(line: &str) -> Option<(String, f32)> {
    // Format: "<NAME> <kind>(<N>)=<value><unit>"
    // Split on '=' to grab the value; first whitespace-separated token is the
    // rail name (with _A or _V suffix).
    let (lhs, rhs) = line.split_once('=')?;
    let name = lhs.split_whitespace().next()?.to_string();
    // rhs ends with 'A' or 'V' (the unit char). Trim it.
    let value_str = rhs.trim();
    let value_str = value_str
        .strip_suffix('A')
        .or_else(|| value_str.strip_suffix('V'))
        .unwrap_or(value_str);
    let value: f32 = value_str.parse().ok()?;
    Some((name, value))
}

// ─────────────────────────────────────────────────────────────────────────
// hwmon scan
// ─────────────────────────────────────────────────────────────────────────

/// Walk /sys/class/hwmon/hwmon* and push everything useful into `out`.
///
/// hwmon naming convention:
///   tempN_input    → millidegrees C
///   inN_input      → millivolts
///   currN_input    → milliamperes
///   powerN_input   → microwatts
///   <prefix>N_label → human-readable label for that channel
fn scan_hwmon(out: &mut Vec<SysSensor>) {
    let Ok(dir) = fs::read_dir("/sys/class/hwmon") else {
        return;
    };
    for entry in dir.flatten() {
        let path = entry.path();
        let chip_name = read_string_file(&path.join("name")).unwrap_or_else(|| "hwmon".into());
        scan_hwmon_chip(&path, &chip_name, out);
    }
}

fn scan_hwmon_chip(path: &Path, chip_name: &str, out: &mut Vec<SysSensor>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    let names: Vec<String> = entries.flatten().filter_map(|e| e.file_name().to_str().map(String::from)).collect();

    for name in &names {
        let Some(stripped) = name.strip_suffix("_input") else {
            continue;
        };
        let (prefix, channel) = split_prefix_channel(stripped);

        let (kind, scale) = match prefix {
            "temp" => (SysSensorKind::Temperature, 1.0 / 1000.0),
            "in" => (SysSensorKind::Voltage, 1.0 / 1000.0),
            "curr" => (SysSensorKind::Current, 1.0 / 1000.0),
            "power" => (SysSensorKind::Power, 1.0 / 1_000_000.0),
            _ => continue,
        };

        let value = match read_int_file(&path.join(name)) {
            Some(v) => v as f32 * scale,
            None => continue,
        };

        // Pair with `<prefix><N>_label` if present, else build "<chip> <prefix><N>".
        let label_file = format!("{}{}_label", prefix, channel);
        let label = if names.iter().any(|n| n == &label_file) {
            read_string_file(&path.join(&label_file)).unwrap_or_else(|| chip_name.into())
        } else {
            format!("{} {}{}", chip_name, prefix, channel)
        };

        out.push(SysSensor { name: label, kind, value });
    }
}

fn split_prefix_channel(s: &str) -> (&str, &str) {
    let split_at = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
    s.split_at(split_at)
}

// ─────────────────────────────────────────────────────────────────────────
// RAPL (Intel/AMD x86)
// ─────────────────────────────────────────────────────────────────────────

fn read_rapl_uj() -> Option<u64> {
    let s = fs::read_to_string("/sys/class/powercap/intel-rapl:0/energy_uj").ok()?;
    s.trim().parse().ok()
}

// ─────────────────────────────────────────────────────────────────────────
// power_supply / batteries
// ─────────────────────────────────────────────────────────────────────────

fn scan_power_supplies() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(dir) = fs::read_dir("/sys/class/power_supply") else {
        return out;
    };
    for entry in dir.flatten() {
        let path = entry.path();
        if let Some(t) = read_string_file(&path.join("type")) {
            if t == "Battery" {
                out.push(path);
            }
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────
// NVMe disk temperatures
// ─────────────────────────────────────────────────────────────────────────

fn scan_nvme_temps(out: &mut Vec<SysSensor>) {
    let Ok(dir) = fs::read_dir("/sys/class/nvme") else {
        return;
    };
    for entry in dir.flatten() {
        let nvme_name = entry.file_name().to_string_lossy().into_owned();
        let candidates = [
            entry.path().join("hwmon0/temp1_input"),
            entry.path().join("hwmon1/temp1_input"),
            entry.path().join("device/hwmon0/temp1_input"),
        ];
        for c in &candidates {
            if let Some(v) = read_int_file(c) {
                out.push(SysSensor {
                    name: format!("NVMe {}", nvme_name),
                    kind: SysSensorKind::Temperature,
                    value: v as f32 / 1000.0,
                });
                break;
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Generic thermal zones (fallback for ARM SoCs without hwmon temp entries)
// ─────────────────────────────────────────────────────────────────────────

/// Best-effort primary host temperature in °C, for the U-STATUS info responder. Scans the kernel
/// thermal zones and prefers a CPU/SoC/package zone, falling back to the hottest reading. Returns
/// `None` where no thermal zones exist (e.g. a macOS dev host).
pub fn cpu_temp_c() -> Option<f32> {
    let dir = fs::read_dir("/sys/class/thermal").ok()?;
    let mut readings: Vec<(String, f32)> = Vec::new();
    for entry in dir.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("thermal_zone") {
            continue;
        }
        let path = entry.path();
        if let Some(millideg) = read_int_file(&path.join("temp")) {
            let kind = read_string_file(&path.join("type")).unwrap_or_default().to_lowercase();
            readings.push((kind, millideg as f32 / 1000.0));
        }
    }
    readings
        .iter()
        .find(|(k, _)| k.contains("cpu") || k.contains("soc") || k.contains("pkg") || k.contains("x86"))
        .map(|(_, t)| *t)
        .or_else(|| {
            readings
                .iter()
                .map(|(_, t)| *t)
                .fold(None, |m, t| Some(m.map_or(t, |x: f32| x.max(t))))
        })
}

/// Best-effort primary (outbound) host IPv4 address, for the U-STATUS info responder. Uses the
/// connect-a-UDP-socket trick: no packet is sent, the kernel just resolves which local interface
/// would route to a public address — which is the hotspot/LAN address the operator wants. Returns
/// `None` when there is no usable route.
pub fn primary_ip() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    if ip.is_unspecified() || ip.is_loopback() {
        return None;
    }
    Some(ip.to_string())
}

fn scan_thermal_zones(out: &mut Vec<SysSensor>) {
    let Ok(dir) = fs::read_dir("/sys/class/thermal") else {
        return;
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("thermal_zone") {
            continue;
        }
        let path = entry.path();
        let zone_type = read_string_file(&path.join("type")).unwrap_or_else(|| name_str.to_string());
        if let Some(millideg) = read_int_file(&path.join("temp")) {
            out.push(SysSensor {
                name: zone_type,
                kind: SysSensorKind::Temperature,
                value: millideg as f32 / 1000.0,
            });
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// File-reading helpers
// ─────────────────────────────────────────────────────────────────────────

fn read_string_file(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_int_file(path: &Path) -> Option<i64> {
    read_string_file(path)?.parse().ok()
}
