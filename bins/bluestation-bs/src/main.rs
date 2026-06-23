use clap::Parser;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use tetra_core::tetra_entities::TetraEntity;
use tetra_entities::net_control::channel::build_all_control_links;
use tetra_entities::net_control::{
    CONTROL_HEARTBEAT_INTERVAL, CONTROL_HEARTBEAT_TIMEOUT, CONTROL_PROTOCOL_VERSION, CommandDispatcher, ControlWorker,
};

use tetra_config::bluestation::{PhyBackend, SharedConfig, StackConfig, parsing};
use tetra_core::{TdmaTime, debug};
use tetra_entities::MessageRouter;
#[cfg(feature = "asterisk")]
use tetra_entities::net_asterisk::entity::AsteriskEntity;
use tetra_entities::net_brew::entity::BrewEntity;
use tetra_entities::net_brew::new_websocket_transport;
use tetra_entities::net_dapnet::spawn_dapnet_worker;
use tetra_entities::net_dashboard::DashboardServer;
use tetra_entities::net_geoalarm::{GeoAlarmSink, spawn_geoalarm_worker};
use tetra_entities::net_snom::{snom_notify_channel, spawn_snom_notify_worker};
use tetra_entities::net_telegram::{TelegramAlertSink, TelegramAlerter, telegram_alert_channel};
use tetra_entities::net_telemetry::worker::TelemetryWorker;
use tetra_entities::net_telemetry::{
    TELEMETRY_HEARTBEAT_INTERVAL, TELEMETRY_HEARTBEAT_TIMEOUT, TELEMETRY_PROTOCOL_VERSION, TelemetrySink, TelemetrySource,
    telemetry_channel,
};
use tetra_entities::network::transports::websocket::{WebSocketTransport, WebSocketTransportConfig};
use tetra_entities::{
    cmce::cmce_bs::CmceBs,
    llc::llc_bs_ms::Llc,
    lmac::lmac_bs::LmacBs,
    mle::mle_bs::MleBs,
    mm::mm_bs::MmBs,
    phy::{components::soapy_dev::RxTxDevSoapySdr, phy_bs::PhyBs},
    sndcp::sndcp_bs::Sndcp,
    umac::umac_bs::UmacBs,
};

/// Result of loading config — either primary or fallback.
enum ConfigLoadResult {
    Primary(StackConfig),
    Fallback {
        config: StackConfig,
        fallback_path: String,
        primary_error: String,
    },
}

/// Try to load the primary config. If it fails, try the fallback
/// (`<config>.fallback` alongside the primary file).
/// Returns Ok(ConfigLoadResult) or exits if both fail.
fn load_config_with_fallback(cfg_path: &str) -> ConfigLoadResult {
    match parsing::from_file(cfg_path) {
        Ok(c) => ConfigLoadResult::Primary(c),
        Err(primary_err) => {
            let primary_err_str = primary_err.to_string();
            eprintln!("WARNING: Failed to load primary config '{}': {}", cfg_path, primary_err_str);

            // Fallback path: same directory, same name + ".fallback"
            let fallback_path = format!("{}.fallback", cfg_path);

            eprintln!("WARNING: Trying fallback config '{}'...", fallback_path);
            match parsing::from_file(&fallback_path) {
                Ok(c) => {
                    eprintln!(
                        "WARNING: Started on FALLBACK config '{}'. Primary config is invalid!",
                        fallback_path
                    );
                    ConfigLoadResult::Fallback {
                        config: c,
                        fallback_path,
                        primary_error: primary_err_str,
                    }
                }
                Err(fallback_err) => {
                    eprintln!("ERROR: Fallback config '{}' also failed: {}", fallback_path, fallback_err);
                    eprintln!("ERROR: No valid config available. Cannot start.");
                    eprintln!("HINT:  Fix '{}' or create a valid fallback at '{}'", cfg_path, fallback_path);
                    std::process::exit(1);
                }
            }
        }
    }
}

fn start_telemetry_worker(cfg: SharedConfig, telemetry_source: TelemetrySource) -> thread::JoinHandle<()> {
    let config = cfg.config();
    let tcfg = config.telemetry.as_ref().unwrap();

    let custom_root_certs = tcfg.ca_cert.as_ref().map(|path| {
        let der_bytes = std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("Failed to read CA certificate from '{}': {}", path, e);
            std::process::exit(1);
        });
        vec![rustls::pki_types::CertificateDer::from(der_bytes)]
    });

    let ws_config = WebSocketTransportConfig {
        host: tcfg.host.clone(),
        port: tcfg.port,
        use_tls: tcfg.use_tls,
        digest_auth_credentials: None,
        basic_auth_credentials: tcfg.credentials.clone(),
        endpoint_path: "/".to_string(),
        subprotocol: Some(TELEMETRY_PROTOCOL_VERSION.to_string()),
        user_agent: format!("BlueStation/{}", tetra_core::STACK_VERSION),
        heartbeat_interval: TELEMETRY_HEARTBEAT_INTERVAL,
        heartbeat_timeout: TELEMETRY_HEARTBEAT_TIMEOUT,
        custom_root_certs,
    };

    thread::spawn(move || {
        let transport = WebSocketTransport::new(ws_config);
        let mut worker = TelemetryWorker::new(telemetry_source, transport);
        worker.run();
    })
}

fn start_control_worker(cfg: SharedConfig, command_dispatchers: HashMap<TetraEntity, CommandDispatcher>) -> thread::JoinHandle<()> {
    let config = cfg.config();
    let ccfg = config.control.as_ref().unwrap();

    let custom_root_certs = ccfg.ca_cert.as_ref().map(|path| {
        let der_bytes = std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("Failed to read CA certificate from '{}': {}", path, e);
            std::process::exit(1);
        });
        vec![rustls::pki_types::CertificateDer::from(der_bytes)]
    });

    let ws_config = WebSocketTransportConfig {
        host: ccfg.host.clone(),
        port: ccfg.port,
        use_tls: ccfg.use_tls,
        digest_auth_credentials: None,
        basic_auth_credentials: ccfg.credentials.clone(),
        endpoint_path: "/".to_string(),
        subprotocol: Some(CONTROL_PROTOCOL_VERSION.to_string()),
        user_agent: format!("BlueStation/{}", tetra_core::STACK_VERSION),
        heartbeat_interval: CONTROL_HEARTBEAT_INTERVAL,
        heartbeat_timeout: CONTROL_HEARTBEAT_TIMEOUT,
        custom_root_certs,
    };

    thread::spawn(move || {
        let transport = WebSocketTransport::new(ws_config);
        let mut worker = ControlWorker::new(command_dispatchers, transport);
        worker.run();
    })
}

/// Start base station stack
fn build_bs_stack(
    cfg: &mut SharedConfig,
    config_path: &str,
) -> (
    MessageRouter,
    Option<TelemetrySource>,
    HashMap<TetraEntity, CommandDispatcher>,
    Option<TelemetrySink>,
) {
    let mut router = MessageRouter::new(cfg.clone());

    // Build telemetry sink/source — always create if either telemetry or dashboard is enabled
    let needs_telemetry = cfg.config().telemetry.is_some()
        || cfg.config().dashboard.is_some()
        || cfg.config().telegram.is_some()
        || cfg.config().geoalarm.enabled
        || cfg.effective_snom_notify().enabled;
    let (tsink, tsource) = if needs_telemetry {
        let (a, b) = telemetry_channel();
        (Some(a), Some(b))
    } else {
        (None, None)
    };

    // Add suitable Phy component based on PhyIo type
    match cfg.config().phy_io.backend {
        PhyBackend::SoapySdr => {
            let rxdev = RxTxDevSoapySdr::with_telemetry(cfg, tsink.clone());
            let phy = PhyBs::new(cfg.clone(), rxdev);
            router.register_entity(Box::new(phy));
        }
        _ => {
            panic!("Unsupported PhyIo type: {:?}", cfg.config().phy_io.backend);
        }
    }

    // Background sys-health worker — reads /sys for temperatures, voltages,
    // currents, power. Universal across host hardware: RPi 5 (full PMIC),
    // RPi 4 (CPU temp), x86 desktop (RAPL + motherboard sensors), laptops
    // (battery). Falls back gracefully if nothing is available.
    if let Some(ref sink) = tsink {
        tetra_entities::sys_telemetry::spawn_sys_health(sink.clone());

        // Background lite stack-health monitor — samples the global health registry and emits a
        // HealthSnapshot through telemetry (→ dashboard tile + Telegram alerts). Tunable via the
        // `[health]` config section; observe-only unless `restart_on_core_stall` is enabled.
        let hcfg = cfg.config().health.clone();
        if hcfg.enabled {
            use std::time::Duration;
            tetra_entities::health::registry().set_brew_configured(cfg.config().brew.is_some());
            tetra_entities::health::spawn_health_monitor(
                sink.clone(),
                tetra_entities::health::HealthMonitorConfig {
                    snapshot_interval: Duration::from_secs(hcfg.snapshot_interval_secs),
                    thresholds: tetra_entities::health::HealthThresholds {
                        core_stall_critical_ms: hcfg.core_stall_secs.saturating_mul(1000),
                        radios_silent_degraded_secs: hcfg.radios_silent_secs,
                        dl_queue_degraded: hcfg.dl_queue_degraded as usize,
                        dl_queue_critical: hcfg.dl_queue_critical as usize,
                        sds_queue_degraded: hcfg.sds_queue_degraded as usize,
                        sds_queue_critical: hcfg.sds_queue_critical as usize,
                    },
                    restart_on_core_stall: hcfg.restart_on_core_stall,
                    restart_after_critical: Duration::from_secs(hcfg.restart_after_critical_secs),
                    restart_cooldown: Duration::from_secs(hcfg.restart_cooldown_secs),
                },
            );
        }
    }

    // Always build control links — dashboard needs them even without external control server
    let (mut c_d, mut c_e) = build_all_control_links();

    // Add remaining components
    let lmac = LmacBs::new(cfg.clone());
    let umac = UmacBs::new(cfg.clone(), tsink.clone());
    let llc = Llc::new(cfg.clone());
    let mle = MleBs::new(cfg.clone());
    let mut mm = MmBs::new(cfg.clone(), tsink.clone(), c_e.remove(&TetraEntity::Mm));
    let sndcp = Sndcp::new(cfg.clone());
    let mut cmce = CmceBs::new(cfg.clone(), tsink.clone(), c_e.remove(&TetraEntity::Cmce));
    // Wire the built-in WX/METAR service's reply channel: its background fetch threads
    // re-inject SendSds commands through the CMCE command dispatcher, same as the dashboard.
    if let Some(d) = c_d.get(&TetraEntity::Cmce) {
        cmce.set_wx_cmd_sender(d.clone_sender());
    }
    // Restart recovery: when enabled, seed MM from the on-disk cache and start the cold-start
    // re-registration sweep. The cache path is the configured override, else
    // `<config-dir>/recovery_cache.json` (the radioid_cache.json convention).
    if cfg.config().recovery.enabled {
        let cache_path = cfg
            .config()
            .recovery
            .cache_path
            .clone()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(config_path)
                    .parent()
                    .map(|d| d.join("recovery_cache.json"))
                    .unwrap_or_else(|| std::path::PathBuf::from("recovery_cache.json"))
            });
        eprintln!(" -> Restart recovery enabled (cache: {})", cache_path.display());
        mm.init_recovery(cache_path);
    }

    router.register_entity(Box::new(lmac));
    router.register_entity(Box::new(umac));
    router.register_entity(Box::new(llc));
    router.register_entity(Box::new(mle));
    router.register_entity(Box::new(mm));
    router.register_entity(Box::new(sndcp));
    router.register_entity(Box::new(cmce));

    // Drop all command links that were not given to a TetraEntity
    for (entity, dispatcher) in c_e.into_iter() {
        drop(dispatcher);
        c_d.remove(&entity);
    }

    // Register Brew entity if enabled
    if let Some(ref brew_cfg) = cfg.config().brew {
        let transport = new_websocket_transport(brew_cfg);
        let mut brew_entity = BrewEntity::new(cfg.clone(), transport);
        if let Some(ref sink) = tsink {
            brew_entity.set_telemetry_sink(sink.clone());
        }
        router.register_entity(Box::new(brew_entity));
        eprintln!(" -> Brew/TetraPack integration enabled");
    }

    // Register Asterisk SIP/RTP entity if enabled. Only compiled in with the
    // `asterisk` feature, which links the native TETRA codec.
    #[cfg(feature = "asterisk")]
    if cfg.config().asterisk.enabled {
        match AsteriskEntity::new(cfg.clone()) {
            Ok(asterisk_entity) => {
                router.register_entity(Box::new(asterisk_entity));
                eprintln!(" -> Asterisk SIP integration enabled");
            }
            Err(err) => {
                panic!("Failed to start Asterisk SIP integration: {}", err);
            }
        }
    }

    // Init network time
    router.set_dl_time(TdmaTime::default());

    (router, tsource, c_d, tsink)
}

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "TETRA BlueStation base station stack",
    long_about = "Runs the TETRA BlueStation base station stack using the provided TOML configuration files"
)]

struct Args {
    /// Config file (required)
    #[arg(help = "TOML config with network/cell parameters")]
    config: String,
}

fn main() {
    eprintln!("░▀█▀░█▀▀░▀█▀░█▀▄░█▀█░░░░░░░░░█▀▀░█░░░█▀█░█░█░█▀▀░▀█▀░█▀█░▀█▀░▀█▀░█▀█░█▀█");
    eprintln!("░░█░░█▀▀░░█░░█▀▄░█▀█░░░▄▄▄░░░█▀▀░█░░░█░█░█▄█░▀▀█░░█░░█▀█░░█░░░█░░█░█░█░█");
    eprintln!("░░▀░░▀▀▀░░▀░░▀░▀░▀░▀░░░░░░░░░▀░░░▀▀▀░▀▀▀░▀░▀░▀▀▀░░▀░░▀░▀░░▀░░▀▀▀░▀▀▀░▀░▀\n");
    eprintln!("  Razvan Zeces / FlowStation.network");
    eprintln!("  https://github.com/razvanzeces/flowstation");
    eprintln!("  Version: {}", tetra_core::STACK_VERSION);

    // Parse command-line arguments
    let args = Args::parse();

    // Load config — tries primary, falls back to <config>.fallback if primary is invalid.
    let (stack_cfg, fallback_info) = match load_config_with_fallback(&args.config) {
        ConfigLoadResult::Primary(c) => (c, None),
        ConfigLoadResult::Fallback {
            config,
            fallback_path,
            primary_error,
        } => (config, Some((fallback_path, primary_error))),
    };

    // Build immutable, cheaply clonable SharedConfig and build the base station stack
    let mut cfg = SharedConfig::from_parts(stack_cfg, None);

    // If the dashboard OR Telegram alerts are enabled, set up the log capture channel BEFORE
    // logging initialises (Telegram forwards WARN/ERROR lines as its critical-status catch-all).
    let dashboard_log_rx = if cfg.config().dashboard.is_some() || cfg.config().telegram.is_some() {
        let (tx, rx) = crossbeam_channel::unbounded::<(String, String)>();
        debug::set_dashboard_log_sender(tx);
        Some(rx)
    } else {
        None
    };

    let _log_guards = debug::setup_logging_default(cfg.config().debug_log.clone());

    // Apply explicit systemd service name from config, if provided.
    // Used by SDS command control (restart/shutdown) and dashboard OTA.
    // Auto-detection from /proc/self/cgroup is still the fallback.
    if let Some(ref service_name) = cfg.config().service_name {
        tetra_entities::service_control::set_configured_service_unit(service_name);
        tracing::info!("Service control: using configured service_name={}", service_name);
    }

    // Log fallback immediately after logging is set up, even without dashboard.
    if let Some((ref fb_path, ref fb_reason)) = fallback_info {
        tracing::warn!(
            "FALLBACK CONFIG ACTIVE: primary config '{}' failed ({}). Running on '{}'.",
            args.config,
            fb_reason,
            fb_path
        );
    }

    let (mut router, tsource, cdispatchers, dapnet_telemetry_sink) = build_bs_stack(&mut cfg, &args.config);
    let dapnet_cmd_tx = cdispatchers.get(&TetraEntity::Cmce).map(|dispatcher| dispatcher.clone_sender());
    let mut dapnet_telegram_sink: Option<TelegramAlertSink> = None;
    #[allow(unused_assignments)]
    let mut geoalarm_sink: Option<GeoAlarmSink> = None;

    // Snom XML NOTIFY worker — off-path; idles when disabled and reads settings live.
    let (snom_sink, snom_source) = snom_notify_channel();
    spawn_snom_notify_worker(cfg.clone(), snom_source);
    let snom_notify_sink = Some(snom_sink);
    if cfg.effective_snom_notify().enabled {
        let snom = cfg.effective_snom_notify();
        eprintln!(
            " -> Snom NOTIFY integration enabled (AMI {}:{}, endpoints={})",
            snom.ami_host,
            snom.ami_port,
            snom.endpoints.join(",")
        );
    }

    // Start Telemetry and Control threads, if enabled
    // If dashboard is also enabled, tee the telemetry events to both.
    if let Some(telemetry_source) = tsource {
        let has_telemetry_server = cfg.config().telemetry.is_some();
        let has_dashboard = cfg.config().dashboard.is_some();
        let has_telegram = cfg.config().telegram.is_some();

        // Telegram alerter — independent of the dashboard. Spawned whenever [telegram_alerts]
        // exists; it idles when alerts are disabled and reads settings live via
        // effective_telegram(), so toggling from the dashboard takes effect without a restart.
        let alert_sink = if has_telegram {
            let (sink, alert_source) = telegram_alert_channel();
            let alert_cfg = cfg.clone();
            let snom_for_alerts = snom_notify_sink.clone();
            thread::Builder::new()
                .name("telegram-alerter".into())
                .spawn(move || {
                    TelegramAlerter::new(alert_cfg, alert_source).with_snom_sink(snom_for_alerts).run();
                })
                .expect("failed to spawn telegram-alerter thread");
            Some(sink)
        } else {
            None
        };
        dapnet_telegram_sink = alert_sink.clone();
        // GeoAlarm worker — fed TETRA LIP positions from the telemetry fan-out below.
        geoalarm_sink = spawn_geoalarm_worker(cfg.clone(), dapnet_cmd_tx.clone(), alert_sink.clone(), snom_notify_sink.clone());

        // Optional dashboard HTTP server.
        let dashboard: Option<std::sync::Arc<DashboardServer>> = if has_dashboard {
            let dash_cfg = cfg.config().dashboard.clone().unwrap();
            let mut dashboard = DashboardServer::new(args.config.clone());

            // Propagate optional source_dir override for OTA updates.
            dashboard.set_source_dir(dash_cfg.source_dir.clone());

            // Propagate optional HTTP Basic Auth credentials.
            if let (Some(user), Some(pass)) = (dash_cfg.username.clone(), dash_cfg.password.clone()) {
                tracing::info!("Dashboard: HTTP Basic Auth enabled (user: {})", user);
                dashboard.set_auth(Some((user, pass)));
            }

            // Optional anonymous read-only public overview (FH-FEAT-033). Inert unless auth is set;
            // must be configured before start(), which captures the flag into the server thread.
            if dash_cfg.public_overview {
                tracing::info!("Dashboard: public read-only overview enabled for anonymous visitors");
            }
            dashboard.set_public_overview(dash_cfg.public_overview);

            // Propagate SharedConfig so the dashboard can read live SDS queue state.
            dashboard.set_shared_config(cfg.clone());

            // Create a control link so dashboard can send commands to CMCE
            let dash_cmd_tx = {
                use tetra_core::tetra_entities::TetraEntity;
                cdispatchers.get(&TetraEntity::Cmce).map(|d| d.clone_sender())
            };

            if let Some(tx) = dash_cmd_tx {
                dashboard.set_cmd_sender(tx);
            }

            // start() must be called before Arc::new() because it takes &mut self
            dashboard.start(&dash_cfg.bind, dash_cfg.port);
            eprintln!(" -> Dashboard enabled on http://{}:{}", dash_cfg.bind, dash_cfg.port);

            // If we started on fallback config, tell the dashboard to show the warning banner.
            if let Some((ref fb_path, ref fb_reason)) = fallback_info {
                let reason = format!(
                    "Primary config '{}' failed to load: {}. Running on fallback '{}'.",
                    args.config, fb_reason, fb_path
                );
                tracing::warn!("{}", reason);
                dashboard.set_fallback_config(reason);
            }

            Some(std::sync::Arc::new(dashboard))
        } else {
            None
        };

        // Capture log lines: push to the dashboard log tab (if present) and forward WARN/ERROR to
        // the Telegram alerter (if present) as its critical-status catch-all. The alerter logs its
        // own send failures at debug! level, so this forward never loops. This thread also drains
        // the log channel even when the dashboard is absent (Telegram-only), so it cannot grow.
        if let Some(log_rx) = dashboard_log_rx {
            let dash_log = dashboard.clone();
            let log_alert = alert_sink.clone();
            thread::Builder::new()
                .name("log-fanout".into())
                .spawn(move || {
                    while let Ok((level, msg)) = log_rx.recv() {
                        // Filter out debug/trace noise.
                        if level == "DEBUG" || level == "TRACE" {
                            continue;
                        }
                        // Filter out TDMA tick noise — thousands per second.
                        if msg.contains("tick dl") || msg.contains("tick ul") || msg.starts_with("--- tick") {
                            continue;
                        }
                        if level == "WARN" || level == "ERROR" {
                            if let Some(s) = &log_alert {
                                s.send_log(level.clone(), msg.clone());
                            }
                        }
                        if let Some(d) = &dash_log {
                            d.push_log(&level, msg);
                        }
                    }
                })
                .expect("failed to spawn log-fanout thread");
        }

        // Single telemetry consumer: fan each event out to the dashboard, the Telegram alerter, and
        // the network telemetry worker — each independently optional.
        let (tee_sink, tee_source) = if has_telemetry_server {
            let (a, b) = telemetry_channel();
            (Some(a), Some(b))
        } else {
            (None, None)
        };
        {
            let dash = dashboard.clone();
            let alert = alert_sink.clone();
            let snom = snom_notify_sink.clone();
            let geoalarm = geoalarm_sink.clone();
            thread::Builder::new()
                .name("telemetry-fanout".into())
                .spawn(move || {
                    use tetra_entities::health::registry as health_registry;
                    use tetra_entities::net_telemetry::TelemetryEvent;
                    // Approximate attached-radio count, maintained from registration telemetry, fed to
                    // the health registry (Radios domain). Re-registrations of an already-known radio
                    // don't emit MsRegistration, so increment/decrement stays balanced.
                    let mut radio_count: usize = 0;
                    loop {
                        match telemetry_source.recv() {
                            Some(event) => {
                                // Feed the lite health registry (cheap, before the fan-out clones).
                                match &event {
                                    TelemetryEvent::BrewConnected { connected, .. } => health_registry().set_brew_up(*connected),
                                    TelemetryEvent::MsRegistration { .. } => {
                                        radio_count += 1;
                                        health_registry().set_registered_radios(radio_count);
                                        health_registry().note_radio_activity();
                                    }
                                    TelemetryEvent::MsDeregistration { .. } | TelemetryEvent::MsTimeoutDrop { .. } => {
                                        radio_count = radio_count.saturating_sub(1);
                                        health_registry().set_registered_radios(radio_count);
                                    }
                                    TelemetryEvent::MsRssi { .. } => health_registry().note_radio_activity(),
                                    _ => {}
                                }
                                // Feed decoded TETRA LIP positions (SDS protocol-id 10, inbound from
                                // a radio) to the GeoAlarm worker so it can geofence them.
                                if let Some(g) = &geoalarm
                                    && let TelemetryEvent::SdsLog {
                                        direction,
                                        source_issi,
                                        protocol_id,
                                        text,
                                        ..
                                    } = &event
                                    && *protocol_id == 10
                                    && direction == "rx"
                                {
                                    g.send_tetra_lip(*source_issi, text);
                                }
                                if let Some(d) = &dash {
                                    d.handle_telemetry(event.clone());
                                }
                                if let Some(s) = &alert {
                                    s.send_event(event.clone());
                                }
                                if let Some(s) = &snom {
                                    s.send_event(event.clone());
                                }
                                if let Some(t) = &tee_sink {
                                    t.send(event);
                                }
                            }
                            None => break,
                        }
                    }
                })
                .expect("failed to spawn telemetry-fanout thread");
        }
        if let Some(tee_source) = tee_source {
            start_telemetry_worker(cfg.clone(), tee_source);
        }
    };

    if cfg.config().control.is_some() {
        start_control_worker(cfg.clone(), cdispatchers);
    };

    // DAPNET worker — off-path; idles when disabled and reads settings live.
    spawn_dapnet_worker(cfg.clone(), dapnet_cmd_tx, dapnet_telegram_sink, dapnet_telemetry_sink);

    // Set up Ctrl+C handler for graceful shutdown.
    // Also installs lifecycle control so RestartService / ShutdownService commands
    // can request shutdown with the correct exit code (75 for restart, signaling
    // systemd to restart us instead of treating it as a normal exit).
    let is_running = Arc::new(AtomicBool::new(true));
    tetra_entities::service_control::install_lifecycle_control(is_running.clone());
    let is_running_clone = is_running.clone();
    ctrlc::set_handler(move || {
        is_running_clone.store(false, Ordering::SeqCst);
    })
    .expect("failed to set Ctrl+C handler");

    // Start the stack
    router.run_stack(None, Some(is_running));

    // router drops here → entities are dropped, networked entities disconnect.
    // If RestartService/ShutdownService was triggered, exit with the requested code
    // so systemd can restart us (exit 75) or stop cleanly (exit 0).
    if let Some(code) = tetra_entities::service_control::requested_exit_code() {
        std::process::exit(code);
    }
}
