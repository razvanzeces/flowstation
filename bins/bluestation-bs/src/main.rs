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

use tetra_config::bluestation::{SharedConfig, StackConfig, parsing};
use tetra_core::{CellId, TdmaTime, debug};
use tetra_entities::{MessageRouter, TetraEntityTrait};
#[cfg(feature = "asterisk")]
use tetra_entities::net_asterisk::entity::AsteriskEntity;
use tetra_entities::net_brew::entity::BrewEntity;
use tetra_entities::net_brew::new_websocket_transport;
use tetra_entities::net_lst_dispatch::{LstDispatchEntity, LstDispatchHandle, codec_available};
use tetra_entities::net_dapnet::spawn_dapnet_worker;
use tetra_entities::net_dashboard::DashboardServer;
use tetra_entities::net_geoalarm::{GeoAlarmSink, spawn_geoalarm_worker};
use tetra_entities::net_site::{CellLink, LocalNetwork, SharedDirectory, SitePorts, SiteSwitch, site_links};
#[cfg(feature = "asterisk")]
use tetra_entities::net_site::SiteRelay;
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

/// True if the dashboard bind address resolves to a loopback interface only.
/// Used to decide whether to warn about an unauthenticated, off-host-reachable dashboard.
/// A non-parseable / hostname bind is treated as non-loopback (warn rather than stay silent).
fn bind_is_loopback(bind: &str) -> bool {
    bind.parse::<std::net::IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

/// Result of loading config — either primary or fallback.
enum ConfigLoadResult {
    Primary(StackConfig),
    Fallback {
        config: StackConfig,
        fallback_path: String,
        primary_error: String,
    },
}

/// Parse a config file and run semantic validation.
/// Used so a primary that *parses* but fails `validate()` still triggers `.fallback`
/// (instead of panicking later in `SharedConfig::from_parts`).
fn try_load_stack_config(path: &str) -> Result<StackConfig, String> {
    let cfg = parsing::from_file(path).map_err(|e| format!("parse error: {e}"))?;
    cfg.validate()
        .map_err(|e| format!("validation error: {e}"))?;
    Ok(cfg)
}

/// Try to load the primary config (parse + validate). If it fails, try the fallback
/// (`<config>.fallback` alongside the primary file) with the same checks.
/// Returns ConfigLoadResult or exits if both fail.
fn load_config_with_fallback(cfg_path: &str) -> ConfigLoadResult {
    match try_load_stack_config(cfg_path) {
        Ok(c) => ConfigLoadResult::Primary(c),
        Err(primary_err) => {
            eprintln!(
                "WARNING: Failed to load primary config '{}': {}",
                cfg_path, primary_err
            );

            let fallback_path = format!("{}.fallback", cfg_path);

            eprintln!("WARNING: Trying fallback config '{}'...", fallback_path);
            match try_load_stack_config(&fallback_path) {
                Ok(c) => {
                    eprintln!(
                        "WARNING: Started on FALLBACK config '{}'. Primary config is invalid!",
                        fallback_path
                    );
                    ConfigLoadResult::Fallback {
                        config: c,
                        fallback_path,
                        primary_error: primary_err,
                    }
                }
                Err(fallback_err) => {
                    eprintln!(
                        "ERROR: Fallback config '{}' also failed: {}",
                        fallback_path, fallback_err
                    );
                    eprintln!("ERROR: No valid config available. Cannot start.");
                    eprintln!(
                        "HINT:  Fix '{}' or create a valid fallback at '{}'",
                        cfg_path, fallback_path
                    );
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

/// Attach PHY after the dashboard is running. Never panics on SDR open failure —
/// degraded mode keeps the web UI / setup wizard available.
/// Returns true when a PHY was registered.
fn try_attach_phy(router: &mut MessageRouter, cfg: &SharedConfig, tsink: Option<TelemetrySink>) -> bool {
    use tetra_config::bluestation::PhyBackend;
    match cfg.config().phy_io.backend {
        PhyBackend::None => {
            tetra_entities::rf_status::set_offline(
                "phy_io.backend = None (setup mode — complete the Setup wizard to enable RF)",
            );
            eprintln!(" -> RF disabled (setup mode). Dashboard available for configuration.");
            false
        }
        PhyBackend::SoapySdr => {
            tetra_entities::rf_status::set_starting("SoapySdr");
            eprintln!(" -> Opening SDR (dashboard already up)…");
            match RxTxDevSoapySdr::with_telemetry(cfg, tsink) {
                Ok(rxdev) => {
                    let phy = PhyBs::new(cfg.clone(), rxdev);
                    router.register_entity(Box::new(phy));
                    tetra_entities::rf_status::set_online("SoapySdr", "SDR open, PHY registered");
                    eprintln!(" -> RF online (SoapySDR)");
                    true
                }
                Err(e) => {
                    tracing::error!("RF degraded — continuing without PHY: {e}");
                    tetra_entities::rf_status::set_error("SoapySdr", e.clone());
                    eprintln!(
                        " -> RF offline: {e}\n    Dashboard remains available. Fix the SDR in Setup, then restart."
                    );
                    false
                }
            }
        }
        other => {
            let msg = format!("Unsupported PhyIo backend: {other:?}");
            tracing::error!("{msg}");
            tetra_entities::rf_status::set_error("unsupported", msg.clone());
            eprintln!(" -> RF offline: {msg}");
            false
        }
    }
}

/// Start one thread per additional cell (`[[cells]]`). Each runs its own radio stack
/// (PHY → CMCE) on its own SDR with a config from [`StackConfig::for_extra_cell`], so the cells
/// are fully independent for now: separate registrations, no Brew/LST/dashboard. Threads stop
/// with the primary via `is_running`.
fn spawn_extra_cells(
    cell_cfgs: Vec<(CellId, SharedConfig)>,
    is_running: Arc<AtomicBool>,
    mut links: HashMap<CellId, CellLink>,
    mut hosts: HashMap<CellId, NetworkHome>,
    tsink: Option<TelemetrySink>,
) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::new();
    for (id, cell_cfg) in cell_cfgs {
        let is_running = is_running.clone();
        let cell_sink = tsink.as_ref().map(|s| s.for_cell(id.0));
        let link = links.remove(&id);
        let home = hosts.remove(&id);
        let spawned = thread::Builder::new().name(format!("{id}")).spawn(move || {
            tetra_entities::cell_context::set_current(id);
            let phy_sink = cell_sink.clone();
            let mut router = build_cell_router(&cell_cfg, link, home, cell_sink);
            eprintln!(" -> {id}: opening SDR…");
            if !try_attach_phy(&mut router, &cell_cfg, phy_sink) {
                // Without a PHY nothing paces the loop, so don't run it; the cell stays down.
                tracing::error!("{id}: no PHY — cell not started");
                return;
            }
            tracing::info!("{id}: cell stack running");
            router.run_stack(None, Some(is_running));
            tracing::info!("{id}: cell stack stopped");
        });
        match spawned {
            Ok(h) => handles.push(h),
            Err(e) => tracing::error!("{id}: failed to spawn cell thread: {e}"),
        }
    }
    handles
}

/// A network whose switch and core link run in one of the additional cells: the network's home
/// cell, when the primary cell does not serve that network. `others` are the network's remaining
/// cells; with none the home cell runs the core link alone, without a switch.
struct NetworkHome {
    name: String,
    ports: Option<SitePorts>,
    others: Vec<(CellId, SharedConfig)>,
    directory: SharedDirectory,
}

/// The entity in a network's Brew slot: its core link, or a sink for a network with no core.
fn network_entity(cfg: &SharedConfig, directory: Option<&SharedDirectory>, tsink: Option<&TelemetrySink>) -> Box<dyn TetraEntityTrait> {
    let name = cfg.config().network.clone();
    match cfg.config().brew.clone() {
        Some(brew_cfg) => {
            let mut brew = BrewEntity::new(cfg.clone(), new_websocket_transport(&brew_cfg));
            if let Some(sink) = tsink {
                brew.set_telemetry_sink(sink.clone());
            }
            if let Some(directory) = directory {
                brew.set_site_directory(directory.clone());
            }
            eprintln!(" -> {}: network {name}: Brew link to {}:{}", cfg.config().cell_id, brew_cfg.host, brew_cfg.port);
            Box::new(brew)
        }
        None => {
            eprintln!(" -> {}: network {name}: local, no core", cfg.config().cell_id);
            Box::new(LocalNetwork::new(name))
        }
    }
}

/// Radio stack of an additional cell: the core entities only, without external control links or
/// restart recovery. `link` connects a linked cell to its network's site switch; `home` makes
/// this cell host its network's switch and core link instead (a network the primary does not
/// serve); `tsink` is the station's telemetry stream tagged for this cell.
fn build_cell_router(cfg: &SharedConfig, link: Option<CellLink>, home: Option<NetworkHome>, tsink: Option<TelemetrySink>) -> MessageRouter {
    let mut router = MessageRouter::new(cfg.clone());
    router.register_entity(Box::new(LmacBs::new(cfg.clone())));
    router.register_entity(Box::new(UmacBs::new(cfg.clone(), tsink.clone())));
    router.register_entity(Box::new(Llc::new(cfg.clone())));
    router.register_entity(Box::new(MleBs::new(cfg.clone())));
    router.register_entity(Box::new(MmBs::new(cfg.clone(), tsink.clone(), None)));
    router.register_entity(Box::new(Sndcp::new(cfg.clone())));
    // CMCE gets its own command link so the WX/METAR responder can send its replies.
    let (c_d, mut c_e) = build_all_control_links();
    let mut cmce = CmceBs::new(cfg.clone(), tsink, c_e.remove(&TetraEntity::Cmce));
    if let Some(d) = c_d.get(&TetraEntity::Cmce) {
        cmce.set_wx_cmd_sender(d.clone_sender());
    }
    router.register_entity(Box::new(cmce));
    if let Some(home) = home {
        // This cell is where its network lives: the core link (or none) and, with more cells on
        // the network, the switch that joins them. The primary's dashboard still sees the link
        // state through the cell's own state (CellsSnapshot), not the station backhaul indicator.
        let entity = network_entity(cfg, Some(&home.directory), None);
        match home.ports {
            Some(ports) => {
                eprintln!(" -> {}: site switch for network {} shared by {} cells", cfg.config().cell_id, home.name, home.others.len() + 1);
                router.register_entity(Box::new(SiteSwitch::new(entity, cfg.clone(), home.others, ports, home.directory)));
            }
            None => router.register_entity(entity),
        }
    } else if let Some(link) = link {
        // Linked cells reach the primary's Asterisk bridge through a second link.
        if cfg.config().asterisk.enabled {
            router.register_entity(Box::new(link.asterisk_link()));
        }
        router.register_entity(Box::new(link));
    }
    router.set_dl_time(TdmaTime::default());
    router
}

/// Start base station stack
/// Site switch ports, the additional cells' configs it keeps in sync, and the radio directory it
/// shares with the Asterisk relay.
type SiteSetup = (SitePorts, Vec<(CellId, SharedConfig)>, SharedDirectory);

/// In a multi-cell station the network entity is wrapped in the site switch so it serves every
/// cell; single-cell stations register it directly, exactly as before.
fn wrap_network_entity(entity: Box<dyn TetraEntityTrait>, cfg: &SharedConfig, site: &mut Option<SiteSetup>) -> Box<dyn TetraEntityTrait> {
    match site.take() {
        Some((ports, extra, directory)) => {
            eprintln!(" -> Site switch: network link shared by {} cells", extra.len() + 1);
            Box::new(SiteSwitch::new(entity, cfg.clone(), extra, ports, directory))
        }
        None => entity,
    }
}

/// The WAP status pages consume live telemetry even on an otherwise headless station.
fn needs_telemetry(cfg: &SharedConfig) -> bool {
    let config = cfg.config();
    config.telemetry.is_some()
        || config.dashboard.is_some()
        || config.telegram.is_some()
        || config.geoalarm.enabled
        || cfg.effective_snom_notify().enabled
        || config.cell.wap_enabled()
        || config.extra_cells.iter().any(|cell| cell.cell.wap_enabled())
}

fn build_bs_stack(
    cfg: &mut SharedConfig,
    config_path: &str,
    lst_handle: Option<LstDispatchHandle>,
    mut site: Option<SiteSetup>,
) -> (
    MessageRouter,
    Option<TelemetrySource>,
    HashMap<TetraEntity, CommandDispatcher>,
    Option<TelemetrySink>,
) {
    let mut router = MessageRouter::new(cfg.clone());

    // WAP status pages need the same events as the optional dashboard and integrations.
    let (tsink, tsource) = if needs_telemetry(cfg) {
        let (a, b) = telemetry_channel();
        // Multi-cell: tag the primary's events with its cell (registrations get an MsCell).
        let a = if cfg.config().extra_cells.is_empty() { a } else { a.for_cell(0) };
        (Some(a), Some(b))
    } else {
        (None, None)
    };

    // PHY / SDR is attached later via `try_attach_phy` so the dashboard can come up
    // even when no radio is present (setup mode) or the open fails.

    // Background sys-health worker — reads /sys for temperatures, voltages,
    // currents, power. Universal across host hardware: RPi 5 (full PMIC),
    // RPi 4 (CPU temp), x86 desktop (RAPL + motherboard sensors), laptops
    // (battery). Falls back gracefully if nothing is available.
    if let Some(ref sink) = tsink {
        tetra_entities::sys_telemetry::spawn_sys_health(sink.clone());
        tetra_entities::net_dashboard::cells::spawn_cells_telemetry(sink.clone(), cfg.clone());

        // Background lite stack-health monitor — samples the global health registry and emits a
        // HealthSnapshot through telemetry (→ dashboard tile + Telegram alerts). Tunable via the
        // `[health]` config section; observe-only unless `restart_on_core_stall` is enabled.
        let hcfg = cfg.config().health.clone();
        if hcfg.enabled {
            use std::time::Duration;
            // The backhaul indicator follows the primary cell's network; other networks' links
            // show per cell in the Cells card.
            tetra_entities::health::registry().set_brew_configured(cfg.config().brew.is_some());
            tetra_entities::health::spawn_health_monitor(
                sink.clone(),
                tetra_entities::health::HealthMonitorConfig {
                    snapshot_interval: Duration::from_secs(hcfg.snapshot_interval_secs),
                    thresholds: tetra_entities::health::HealthThresholds {
                        core_stall_critical_ms: hcfg.core_stall_secs.saturating_mul(1000),
                        radios_silent_degraded_secs: hcfg.radios_silent_secs,
                        periodic_registration_secs: cfg.config().cell.periodic_registration_secs as u64,
                        dl_queue_degraded: hcfg.dl_queue_degraded as usize,
                        dl_queue_critical: hcfg.dl_queue_critical as usize,
                        sds_queue_degraded: hcfg.sds_queue_degraded as usize,
                        sds_queue_critical: hcfg.sds_queue_critical as usize,
                    },
                    restart_on_core_stall: hcfg.restart_on_core_stall,
                    restart_after_critical: Duration::from_secs(hcfg.restart_after_critical_secs),
                    restart_cooldown: Duration::from_secs(hcfg.restart_cooldown_secs),
                    // The cooldown has to survive the restart it triggers, otherwise a stall that
                    // reproduces at boot reboots forever. Stamp file next to the config, same
                    // convention as recovery_cache.json.
                    restart_state_path: Some(
                        std::path::Path::new(config_path)
                            .parent()
                            .map(|d| d.join("health_restart_state"))
                            .unwrap_or_else(|| std::path::PathBuf::from("health_restart_state")),
                    ),
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

    // Multi-cell: the Asterisk relay's side of the site links (taken before the switch gets them).
    let asterisk_relay = site
        .as_mut()
        .and_then(|(ports, extra, directory)| Some((ports.take_asterisk()?, extra.clone(), directory.clone())));

    // Register Brew XOR LST dispatch (same TetraEntity::Brew slot for CMCE routing).
    let lst_enabled = cfg
        .config()
        .lst_dispatch
        .as_ref()
        .is_some_and(|l| l.enabled);
    if lst_enabled && cfg.config().brew.is_some() {
        panic!("Invalid config: [brew] and lst_dispatch.enabled are mutually exclusive");
    }
    if let Some(handle) = lst_handle {
        if !lst_enabled {
            tracing::warn!("LST handle provided but lst_dispatch not enabled — ignoring");
        } else {
            let entity = LstDispatchEntity::new(cfg.clone(), handle);
            router.register_entity(wrap_network_entity(Box::new(entity), cfg, &mut site));
            eprintln!(" -> LST Dispatch (local console) enabled");
        }
    } else if cfg.config().brew.is_some() {
        let directory = site.as_ref().map(|(_, _, d)| d.clone());
        let brew_entity = network_entity(cfg, directory.as_ref(), tsink.as_ref());
        router.register_entity(wrap_network_entity(brew_entity, cfg, &mut site));
        eprintln!(" -> Brew/TetraPack integration enabled");
    } else if lst_enabled {
        eprintln!(" -> WARNING: lst_dispatch.enabled but no handle wired — dispatch inactive");
    } else if site.is_some() {
        // A local network of several cells: the switch joins them, nothing leaves the station.
        let entity = network_entity(cfg, None, None);
        router.register_entity(wrap_network_entity(entity, cfg, &mut site));
    }

    // Register Asterisk SIP/RTP entity if enabled. Only compiled in with the
    // `asterisk` feature, which links the native TETRA codec.
    #[cfg(feature = "asterisk")]
    if cfg.config().asterisk.enabled {
        match AsteriskEntity::new(cfg.clone()) {
            Ok(asterisk_entity) => {
                // Multi-cell: wrapped so radios on every cell can use the SIP bridge.
                let entity: Box<dyn TetraEntityTrait> = match asterisk_relay {
                    Some((ports, extra, directory)) => Box::new(SiteRelay::new(Box::new(asterisk_entity), cfg, &extra, ports, directory)),
                    None => Box::new(asterisk_entity),
                };
                router.register_entity(entity);
                eprintln!(" -> Asterisk SIP integration enabled");
            }
            Err(err) => {
                panic!("Failed to start Asterisk SIP integration: {}", err);
            }
        }
    }

    #[cfg(not(feature = "asterisk"))]
    let _ = asterisk_relay;

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
    eprintln!("  {} — improved by Aitor, EA4HBL", tetra_core::PRODUCT_NAME);
    eprintln!("  {}", tetra_core::PRODUCT_REPO_URL);
    eprintln!(
        "  Version: {} ({})",
        tetra_core::STACK_VERSION,
        tetra_core::VERSION_BASED_ON
    );
    eprintln!("  Upstream credit: Razvan Zeces / FlowStation.network");

    // Parse command-line arguments
    let args = Args::parse();

    // OTA / upgrades: rewrite legacy dashboard ports (8080→80, ensure https_port=443)
    // before parse so new installs and old boxes share the same defaults.
    {
        use tetra_entities::net_dashboard::dashboard_ports::migrate_legacy_dashboard_ports;
        migrate_legacy_dashboard_ports(&args.config);
        migrate_legacy_dashboard_ports(&format!("{}.fallback", args.config));
    }

    // Load config — parse+validate primary; on failure try <config>.fallback the same way.
    let (mut stack_cfg, fallback_info) = match load_config_with_fallback(&args.config) {
        ConfigLoadResult::Primary(c) => (c, None),
        ConfigLoadResult::Fallback {
            config,
            fallback_path,
            primary_error,
        } => (config, Some((fallback_path, primary_error))),
    };
    // Multi-cell: every cell advertises its siblings as neighbours for reselection.
    stack_cfg.add_sibling_neighbours();

    // Build immutable, cheaply clonable SharedConfig and build the base station stack. The
    // primary config was already parsed+validated by load_config_with_fallback, so this should
    // not fail; exit cleanly rather than panic if it somehow does.
    let mut cfg = match SharedConfig::try_from_parts(stack_cfg, None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: Invalid stack configuration: {e}");
            std::process::exit(1);
        }
    };

    // If the dashboard OR Telegram alerts are enabled, set up the log capture channel BEFORE
    // logging initialises (Telegram forwards WARN/ERROR lines as its critical-status catch-all).
    let dashboard_log_rx = if cfg.config().dashboard.is_some() || cfg.config().telegram.is_some() {
        let (tx, rx) = crossbeam_channel::bounded::<(String, String)>(4096);
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


    let lst_handle = {
        let lst_on = cfg
            .config()
            .lst_dispatch
            .as_ref()
            .is_some_and(|l| l.enabled);
        if lst_on && cfg.config().brew.is_none() {
            let issi = cfg.config().lst_dispatch.as_ref().unwrap().operator_issi;
            Some(LstDispatchHandle::new(issi, codec_available()))
        } else {
            None
        }
    };

    // Multi-cell with a network link: one site switch in the primary router, one link per
    // additional cell. Without Brew/LST there is nothing to share and cells stay independent.
    // Each additional cell's config (derived from its [[cells]] entry) is built here so the
    // switch can keep the cells' network link state in sync with the primary.
    let extra_cell_cfgs: Vec<(CellId, SharedConfig)> = {
        let stack_config = cfg.config();
        stack_config
            .extra_cells
            .iter()
            .filter_map(|c| {
                let cell_cfg = stack_config.for_extra_cell(c.id)?;
                // A bad extra cell must not crash the whole station — skip it with a warning.
                match SharedConfig::try_from_parts(cell_cfg, None) {
                    Ok(sc) => Some((c.id, sc)),
                    Err(e) => {
                        eprintln!("WARNING: skipping extra cell {:?}: {e}", c.id);
                        None
                    }
                }
            })
            .collect()
    };
    tetra_entities::net_site::register_extra_cells(extra_cell_cfgs.clone());
    // One switch per linked network, hosted by the network's home cell: the primary's goes
    // into its router here (`site`), the others into their home cell's router (`network_homes`).
    // A network of one cell with a core runs its link in that cell, without a switch.
    let mut site: Option<SiteSetup> = None;
    let mut cell_links: HashMap<CellId, CellLink> = HashMap::new();
    let mut network_homes: HashMap<CellId, NetworkHome> = HashMap::new();
    {
        let stack_config = cfg.config();
        let cell_cfg = |id: CellId| extra_cell_cfgs.iter().find(|(c, _)| *c == id).cloned();
        for network in &stack_config.networks {
            let Some(home) = stack_config.network_home_cell(&network.name) else { continue };
            let others: Vec<(CellId, SharedConfig)> = stack_config
                .network_cells(&network.name)
                .into_iter()
                .filter(|&id| id != home)
                .filter_map(cell_cfg)
                .collect();
            let linked = stack_config.network_is_linked(&network.name);
            let ports = if linked {
                let other_ids: Vec<CellId> = others.iter().map(|(id, _)| *id).collect();
                let (ports, links) = site_links(&other_ids);
                cell_links.extend(links);
                Some(ports)
            } else {
                None
            };
            let directory = SharedDirectory::default();
            if home.is_primary() {
                if let Some(ports) = ports {
                    site = Some((ports, others, directory));
                }
            } else if cell_cfg(home).is_some() && (ports.is_some() || network.brew.is_some()) {
                network_homes.insert(home, NetworkHome { name: network.name.clone(), ports, others, directory });
            }
        }
    }

    let (mut router, tsource, cdispatchers, dapnet_telemetry_sink) =
        build_bs_stack(&mut cfg, &args.config, lst_handle.clone(), site);
    // Clone for PHY attach after the dashboard is listening (degraded boot).
    let phy_tsink = dapnet_telemetry_sink.clone();
    // Additional cells report registrations etc. through the same stream, tagged per cell.
    let cell_tsink = dapnet_telemetry_sink.clone();
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
            } else if !bind_is_loopback(&dash_cfg.bind) {
                // No credentials AND a non-loopback bind: the dashboard is reachable from the
                // network with no login. The request handler still hard-gates every mutating
                // endpoint to localhost in this state, but read-only views are exposed, so warn
                // loudly. Set both username and password to require login, or bind to 127.0.0.1.
                tracing::warn!(
                    "Dashboard is UNAUTHENTICATED and bound to {} (reachable off-host). \
                     Set [dashboard] username+password, or bind to 127.0.0.1. \
                     Until then, mutating requests are restricted to localhost.",
                    dash_cfg.bind
                );
            }

            // Optional anonymous read-only public overview (FH-FEAT-033). Inert unless auth is set;
            // must be configured before start(), which captures the flag into the server thread.
            if dash_cfg.public_overview {
                tracing::info!("Dashboard: public read-only overview enabled for anonymous visitors");
            }
            dashboard.set_public_overview(dash_cfg.public_overview);

            // Propagate SharedConfig so the dashboard can read live SDS queue state.
            dashboard.set_shared_config(cfg.clone());

            // Dashboard commands are sent directly to their owning entity's control channel.
            // CMCE handles SDS/call/emergency commands; MM handles DGNA and SCK OTAR.
            let dash_cmce_cmd_tx = cdispatchers
                .get(&TetraEntity::Cmce)
                .map(|dispatcher| dispatcher.clone_sender());
            let dash_mm_cmd_tx = cdispatchers
                .get(&TetraEntity::Mm)
                .map(|dispatcher| dispatcher.clone_sender());

            if let Some(tx) = dash_cmce_cmd_tx {
                dashboard.set_cmd_sender(tx);
            }
            if let Some(tx) = dash_mm_cmd_tx {
                dashboard.set_mm_cmd_sender(tx);
            }

            if let Some(ref h) = lst_handle {
                dashboard.set_lst_handle(h.clone());
            }

            // start() must be called before Arc::new() because it takes &mut self
            dashboard.start(&dash_cfg.bind, dash_cfg.port, dash_cfg.https_port);
            if dash_cfg.port == 0 {
                eprintln!(
                    " -> Dashboard HTTPS on https://{}:{} (HTTP redirect off)",
                    dash_cfg.bind, dash_cfg.https_port
                );
            } else {
                eprintln!(
                    " -> Dashboard HTTPS on https://{}:{} (HTTP :{} redirects to HTTPS)",
                    dash_cfg.bind, dash_cfg.https_port, dash_cfg.port
                );
            }

            // If we started on fallback config, tell the dashboard to show the warning banner.
            if let Some((ref fb_path, ref fb_reason)) = fallback_info {
                let reason = format!(
                    "Primary config '{}' failed ({}). Running on fallback '{}'. Fix the primary under Config, then Restart.",
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
                    use tetra_entities::net_geoalarm::parse_lip_position_text;
                    use tetra_entities::net_telemetry::TelemetryEvent;
                    use tetra_entities::radio_directory;
                    // Approximate attached-radio count, maintained from registration telemetry, fed to
                    // the health registry (Radios domain); see `attached_radios_after`.
                    let mut radio_count: usize = 0;
                    loop {
                        match telemetry_source.recv() {
                            Some(event) => {
                                // Feed the lite health registry (cheap, before the fan-out clones).
                                match &event {
                                    TelemetryEvent::BrewConnected { connected, .. } => health_registry().set_brew_up(*connected),
                                    TelemetryEvent::MsRegistration { issi } => {
                                        radio_count = health_registry::attached_radios_after(radio_count, &event);
                                        health_registry().set_registered_radios(radio_count);
                                        health_registry().note_radio_activity();
                                        radio_directory::on_register(*issi);
                                    }
                                    TelemetryEvent::MsDeregistration { issi } | TelemetryEvent::MsTimeoutDrop { issi } => {
                                        radio_count = health_registry::attached_radios_after(radio_count, &event);
                                        health_registry().set_registered_radios(radio_count);
                                        radio_directory::on_deregister(*issi);
                                    }
                                    // Left for another cell or station: no longer attached to that cell;
                                    // the directory drops it only when it left the station.
                                    TelemetryEvent::MsMovedAway { issi, cell } => {
                                        radio_count = health_registry::attached_radios_after(radio_count, &event);
                                        health_registry().set_registered_radios(radio_count);
                                        radio_directory::on_moved_away(*issi, *cell);
                                    }
                                    TelemetryEvent::MsCell { issi, cell } => radio_directory::on_cell(*issi, *cell),
                                    TelemetryEvent::MsRssi { issi, rssi_dbfs } => {
                                        health_registry().note_radio_activity();
                                        radio_directory::on_rssi(*issi, *rssi_dbfs);
                                    }
                                    TelemetryEvent::MsGroupsSnapshot { issi, gssis } => radio_directory::set_groups(*issi, gssis),
                                    TelemetryEvent::MsGroupAttach { issi, gssis } => radio_directory::attach_groups(*issi, gssis),
                                    TelemetryEvent::MsGroupDetach { issi, gssis } => radio_directory::detach_groups(*issi, gssis),
                                    TelemetryEvent::SdsLog { .. } => health_registry().note_sds(),
                                    _ => {}
                                }
                                // Feed decoded TETRA LIP positions (SDS protocol-id 10, inbound from
                                // a radio) to GeoAlarm and the dashboard Geo store (Home + LST).
                                if let TelemetryEvent::SdsLog {
                                    direction,
                                    source_issi,
                                    protocol_id,
                                    text,
                                    ..
                                } = &event
                                    && *protocol_id == 10
                                    && direction == "rx"
                                {
                                    if let Some(g) = &geoalarm {
                                        g.send_tetra_lip(*source_issi, text);
                                    }
                                    if let Some(d) = &dash
                                        && let Some((lat, lon)) = parse_lip_position_text(text)
                                        // Skip null-island / init handshake zeros.
                                        && !(lat.abs() < 1e-9 && lon.abs() < 1e-9)
                                    {
                                        d.note_lip_position(*source_issi, lat, lon);
                                    }
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
    // Soft Shutdown (exit 0) enters standby: dashboard HTTP stays up until Start/Restart
    // requests exit 75, or Ctrl+C ends the process.
    let is_running = Arc::new(AtomicBool::new(true));
    let ctrlc_stop = Arc::new(AtomicBool::new(false));
    tetra_entities::service_control::install_lifecycle_control(is_running.clone());
    let is_running_clone = is_running.clone();
    let ctrlc_stop_clone = ctrlc_stop.clone();
    ctrlc::set_handler(move || {
        is_running_clone.store(false, Ordering::SeqCst);
        ctrlc_stop_clone.store(true, Ordering::SeqCst);
    })
    .expect("failed to set Ctrl+C handler");

    // Open SDR / register PHY only after the dashboard (if any) is already listening,
    // so a missing radio never blocks the setup wizard.
    try_attach_phy(&mut router, &cfg, phy_tsink);

    // Multi-cell: additional cells run independently for now (multi-cell plan, phase 2).
    let extra_cell_threads = spawn_extra_cells(extra_cell_cfgs, is_running.clone(), cell_links, network_homes, cell_tsink);
    if !extra_cell_threads.is_empty() {
        tracing::warn!(
            "{} additional cell(s) started — calls and SDS are routed across cells through the site switch",
            extra_cell_threads.len()
        );
    }

    // Start the stack
    router.run_stack(None, Some(is_running));

    // The extra cells stop on the same `is_running` flag. Wait for them to close their SDRs before
    // this thread exits or closes its own: libiio segfaults in closeStream when two Pluto streams
    // close concurrently, or when the process exits while a cell is still closing.
    for handle in extra_cell_threads {
        if handle.join().is_err() {
            tracing::error!("cell thread panicked during shutdown");
        }
    }

    // Keep `router` alive while we may enter soft standby so the dashboard thread
    // (and SharedConfig) remain available for Arrancar / config / OTA.
    match tetra_entities::service_control::requested_exit_code() {
        Some(75) => {
            // Restart / Start / OTA — systemd brings a fresh process.
            std::process::exit(75);
        }
        Some(0) => {
            eprintln!(
                "\n[INFO] Soft shutdown — stack stopped, dashboard remains available.\n\
                 Use System → Arrancar (or Restart) to bring the station back."
            );
            tracing::warn!("Service control: entering standby (dashboard still serving)");
            loop {
                if ctrlc_stop.load(Ordering::SeqCst) {
                    tracing::info!("Standby interrupted by Ctrl+C — exiting");
                    break;
                }
                if let Some(75) = tetra_entities::service_control::requested_exit_code() {
                    tracing::info!("Standby: start/restart requested — exiting 75 for systemd");
                    std::process::exit(75);
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
        _ => {
            // Ctrl+C or other clean stop while the stack was running.
        }
    }
}

#[cfg(test)]
mod telemetry_tests {
    use super::*;

    fn headless_config() -> StackConfig {
        let mut config = parsing::from_toml_str(include_str!("../../../example_config/config.toml")).expect("example config parses");
        config.phy_io.backend = tetra_config::bluestation::PhyBackend::None;
        config.dashboard = None;
        config.telemetry = None;
        config.telegram = None;
        config.geoalarm.enabled = false;
        config.snom_notify.enabled = false;
        config.cell.wap_ip = None;
        config.extra_cells.clear();
        config
    }

    fn wap_profile() -> tetra_config::bluestation::CfgWapIp {
        tetra_config::bluestation::CfgWapIp {
            enabled: true,
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
            max_request_payload_bytes: 1024,
        }
    }

    #[test]
    fn headless_without_consumers_needs_no_telemetry() {
        let cfg = SharedConfig::try_from_parts(headless_config(), None).unwrap();
        assert!(!needs_telemetry(&cfg));
    }

    #[test]
    fn wap_only_station_starts_live_telemetry() {
        let mut config = headless_config();
        config.cell.wap_ip = Some(wap_profile());
        let cfg = SharedConfig::try_from_parts(config, None).unwrap();
        assert!(needs_telemetry(&cfg));
    }

    #[test]
    fn wap_on_an_extra_cell_also_needs_telemetry() {
        let mut config = headless_config();
        let mut cell = config.cell.clone();
        cell.main_carrier += 1;
        cell.wap_ip = Some(wap_profile());
        config.extra_cells.push(tetra_config::bluestation::CfgExtraCell { id: CellId(1), cell, soapysdr: None, network: None });
        let cfg = SharedConfig::try_from_parts(config, None).unwrap();
        assert!(needs_telemetry(&cfg));
    }
}
