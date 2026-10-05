//! Station status for the WAP pages, from the lite health registry.
//!
//! The registry has no call counters, so the call fields stay at 0; the radio count is 0 while
//! telemetry is off (it is only fed from the telemetry fan-out).

use tetra_config::bluestation::StackConfig;

use crate::health::{HealthDomain, HealthLevel, HealthThresholds, registry};
use crate::sndcp::wap_status::WapStatusSnapshot;

// The WAP pages are in Spanish. The health names themselves (`as_str`) stay as they are: the
// dashboard, the telemetry and Telegram use them.

fn overall_es(level: HealthLevel) -> &'static str {
    match level {
        HealthLevel::Ok => "OK",
        HealthLevel::Degraded => "DEGRADADO",
        HealthLevel::Critical => "CRÍTICO",
    }
}

fn level_es(level: HealthLevel) -> &'static str {
    match level {
        HealthLevel::Ok => "OK",
        HealthLevel::Degraded => "degradado",
        HealthLevel::Critical => "crítico",
    }
}

fn domain_es(domain: HealthDomain) -> &'static str {
    match domain {
        HealthDomain::Service => "Servicio",
        HealthDomain::Backhaul => "Enlace Brew",
        HealthDomain::Radios => "Radios",
        HealthDomain::Congestion => "Congestión",
    }
}

/// Health thresholds from `[health]`, as the health monitor builds them.
pub fn thresholds(cfg: &StackConfig) -> HealthThresholds {
    let h = &cfg.health;
    HealthThresholds {
        core_stall_critical_ms: h.core_stall_secs.saturating_mul(1000),
        radios_silent_degraded_secs: h.radios_silent_secs,
        periodic_registration_secs: cfg.cell.periodic_registration_secs as u64,
        dl_queue_degraded: h.dl_queue_degraded as usize,
        dl_queue_critical: h.dl_queue_critical as usize,
        sds_queue_degraded: h.sds_queue_degraded as usize,
        sds_queue_critical: h.sds_queue_critical as usize,
    }
}

pub fn station_snapshot(thresholds: &HealthThresholds) -> WapStatusSnapshot {
    let reg = registry();
    let health = reg.snapshot(thresholds);
    let overall = overall_es(health.overall).to_string();
    WapStatusSnapshot {
        title: "FlowStation".to_string(),
        stack_version: format!("v{}", env!("CARGO_PKG_VERSION")),
        service_state: overall.clone(),
        registered_ms: reg.registered_radios(),
        active_calls: 0,
        active_group_calls: 0,
        active_private_calls: 0,
        queued_sds: reg.sds_queue_depth(),
        uptime_secs: health.uptime_secs,
        last_activity: None,
        health_summary: Some(overall),
        health_lines: health
            .domains
            .iter()
            .map(|d| format!("{}: {}", domain_es(d.domain), level_es(d.level)))
            .collect(),
        radio_lines: Vec::new(),
        call_lines: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sndcp::wap_status::{render_wml2_status, render_wml2_status_sector};

    #[test]
    fn snapshot_without_telemetry_renders() {
        let snap = station_snapshot(&HealthThresholds::default());
        assert_eq!(snap.title, "FlowStation");
        assert!(snap.stack_version.starts_with('v'));
        assert_eq!(snap.active_calls, 0);
        assert!(!snap.health_lines.is_empty());
        let page = render_wml2_status(&snap, 2048).expect("status page renders");
        assert!(page.contains("FlowStation"));
    }

    #[test]
    fn status_blocks_are_in_spanish_and_small() {
        let snap = station_snapshot(&HealthThresholds::default());
        assert!(
            ["OK", "DEGRADADO", "CRÍTICO"].contains(&snap.service_state.as_str()),
            "{}",
            snap.service_state
        );
        let summary = render_wml2_status_sector(&snap, 8192, 0).unwrap();
        let health = render_wml2_status_sector(&snap, 8192, 1).unwrap();
        for page in [&summary, &health] {
            for english in [
                "Summary",
                "Health",
                "Block",
                "Next",
                "Prev",
                "Top",
                "State",
                "Version ",
                "Uptime",
                "service",
                "backhaul",
                "congestion",
                "degraded",
                "critical",
                " G ",
                " P ",
            ] {
                assert!(!page.contains(english), "{english:?} in {page}");
            }
            assert!(page.contains("<a href=\"/\">Inicio</a>"), "{page}");
            // Both blocks fit in two SAR packets of a 576-byte MTU, like the home page.
            assert!(page.len() <= 1000, "{} bytes: {page}", page.len());
        }
        assert!(
            summary.contains("FlowStation: Resumen") && summary.contains("Bloque 1/2"),
            "{summary}"
        );
        assert!(summary.contains("<br />Estado ") && summary.contains("<br />Versión v") && summary.contains("<br />Activa "));
        assert!(summary.contains("<br />Radios ") && summary.contains(" SDS "), "{summary}");
        assert!(
            health.contains("FlowStation: Salud") && health.contains("<br />General "),
            "{health}"
        );
        // Every health line is shown, congestion (the last one) included.
        for domain in ["Servicio: ", "Enlace Brew: ", "Radios: ", "Congestión: "] {
            assert!(health.contains(&format!("<br />{domain}")), "{domain} in {health}");
        }
    }
}
