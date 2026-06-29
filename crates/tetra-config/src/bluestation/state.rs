use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use tetra_core::TimeslotAllocator;

/// A one-shot or repeating SDS broadcast message injected at runtime via the dashboard.
///
/// Each message is broadcast to all MSs on the cell (GSSI 0xFFFFFF) using the same
/// SDS-TL TRANSFER mechanism as Home Mode Display. Messages are transmitted at the
/// `home_mode_display` interval (or `sds_broadcast` interval if that is configured),
/// round-robining with the static PID-220 callsign text so neither displaces the other.
///
/// - `repeat_count = 0` → repeats indefinitely until explicitly deleted.
/// - `repeat_count > 0` → auto-removed after that many transmissions.
#[derive(Debug, Clone)]
pub struct LiveSdsMessage {
    /// Unique ID (monotonically incrementing, assigned by the stack).
    pub id: u32,
    /// Text to broadcast (UTF-8; encoded as ISO-8859-1 on TX, unknown chars → '?').
    pub text: String,
    /// SDS protocol ID. Defaults to 220 so it appears on the radio home screen.
    pub protocol_id: u8,
    /// Source ISSI shown on the radio. Defaults to 16777215 (0xFFFFFF, "network").
    pub source_issi: u32,
    /// 0 = repeat forever; >0 = auto-remove after this many transmissions.
    pub repeat_count: u32,
    /// Number of times this message has been transmitted so far.
    pub sent_count: u32,
}

#[derive(Debug, Clone)]
pub struct Subscriber {
    pub issi: u32,
    // Set of attached GSSIs
    pub attached_groups: HashSet<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DgnaGroup {
    pub gssi: u32,
    pub mnemonic: Option<String>,
    pub attachment_mode: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceGroup {
    pub gssi: u32,
    pub mnemonic: Option<String>,
    pub attachment_mode: Option<u8>,
    pub is_dynamic: bool,
    pub is_attached: bool,
}

/// Centralized subscriber registry tracking locally registered ISSIs and their group affiliations.
#[derive(Debug, Clone)]
pub struct SubscriberRegistry {
    /// Registered ISSIs → Subscriber information
    subscribers: HashMap<u32, Subscriber>,
    /// Set of all GSSIs with at least one local affiliate
    all_attached_groups: HashSet<u32>,
    /// DGNA groups remembered per device, independent of current affiliation state.
    dgna_groups: HashMap<u32, BTreeMap<u32, DgnaGroup>>,
}

impl Default for SubscriberRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SubscriberRegistry {
    pub fn new() -> Self {
        Self {
            subscribers: HashMap::new(),
            all_attached_groups: HashSet::new(),
            dgna_groups: HashMap::new(),
        }
    }

    pub fn is_registered(&self, issi: u32) -> bool {
        self.subscribers.contains_key(&issi)
    }

    /// Tolerant registration; if ISSI already registered, we overwrite it with a fresh Subscriber struct
    pub fn register(&mut self, issi: u32) {
        self.deregister(issi); // Clean up any existing registration to prevent stale affiliations
        self.subscribers.insert(
            issi,
            Subscriber {
                issi,
                attached_groups: HashSet::new(),
            },
        );
    }

    /// Gets mutable ref to subscriber. If not registered, a default Subscriber is inserted.
    pub fn get_subscriber_mut(&mut self, issi: u32) -> &mut Subscriber {
        self.subscribers.entry(issi).or_insert_with(|| Subscriber {
            issi,
            attached_groups: HashSet::new(),
        })
    }

    /// Deregister an ISSI, removing it from the registry and cleaning up any group affiliations
    pub fn deregister(&mut self, issi: u32) {
        if let Some(subscriber) = self.subscribers.remove(&issi) {
            // Clean up global group affiliations for this subscriber
            for gssi in &subscriber.attached_groups {
                // Check if any other subscriber is still affiliated with this group
                let still_has_members = self.subscribers.values().any(|s| s.attached_groups.contains(gssi));
                if !still_has_members {
                    self.all_attached_groups.remove(gssi);
                }
            }
        }
    }

    /// Add GSSI to subscriber's attached groups and global set
    pub fn affiliate(&mut self, issi: u32, gssi: u32) {
        let subscriber = self.get_subscriber_mut(issi);
        subscriber.attached_groups.insert(gssi);
        self.all_attached_groups.insert(gssi);
    }

    /// Remove GSSI from subscriber's attached groups. Update global set if no more subscribers are affiliated with this GSSI.
    pub fn deaffiliate(&mut self, issi: u32, gssi: u32) {
        let subscriber = self.get_subscriber_mut(issi);
        if subscriber.attached_groups.remove(&gssi) {
            // Check if any other subscriber is still affiliated with this group
            let still_has_members = self.subscribers.values().any(|s| s.attached_groups.contains(&gssi));
            if !still_has_members {
                self.all_attached_groups.remove(&gssi);
            }
        }
    }

    /// Check if any subscriber is affiliated with the given GSSI
    pub fn has_group_members(&self, gssi: u32) -> bool {
        self.all_attached_groups.contains(&gssi)
    }

    /// Returns all currently registered ISSIs.
    ///
    /// Used by BrewEntity after Brew reconnection to issue D-LOCATION-UPDATE-COMMAND
    /// to all locally registered MS, forcing them to re-affiliate with the BS.
    /// Without this, MS units that were registered before a Brew disconnect believe
    /// they are still affiliated and do not re-register, causing PTT denial until
    /// they are manually power-cycled or the BS service is restarted.
    pub fn all_registered_issis(&self) -> impl Iterator<Item = u32> + '_ {
        self.subscribers.keys().copied()
    }

    /// Groups the given ISSI is currently affiliated to (empty if not registered).
    /// Used by the SDS path to reach a member of an active group call on the group's
    /// traffic timeslot.
    pub fn attached_groups_of(&self, issi: u32) -> Vec<u32> {
        self.subscribers
            .get(&issi)
            .map(|s| s.attached_groups.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn remember_dgna_group(&mut self, issi: u32, gssi: u32, mnemonic: Option<String>, attachment_mode: u8) {
        self.dgna_groups.entry(issi).or_default().insert(
            gssi,
            DgnaGroup {
                gssi,
                mnemonic,
                attachment_mode,
            },
        );
    }

    pub fn forget_dgna_group(&mut self, issi: u32, gssi: u32) -> bool {
        let Some(groups) = self.dgna_groups.get_mut(&issi) else {
            return false;
        };
        let removed = groups.remove(&gssi).is_some();
        if groups.is_empty() {
            self.dgna_groups.remove(&issi);
        }
        removed
    }

    pub fn has_dgna_group(&self, issi: u32, gssi: u32) -> bool {
        self.dgna_groups.get(&issi).is_some_and(|groups| groups.contains_key(&gssi))
    }

    pub fn dgna_groups_of(&self, issi: u32) -> Vec<DgnaGroup> {
        self.dgna_groups
            .get(&issi)
            .map(|groups| groups.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn device_groups_of(&self, issi: u32) -> Vec<DeviceGroup> {
        let mut groups = BTreeMap::<u32, DeviceGroup>::new();

        if let Some(subscriber) = self.subscribers.get(&issi) {
            for &gssi in &subscriber.attached_groups {
                groups.insert(
                    gssi,
                    DeviceGroup {
                        gssi,
                        mnemonic: None,
                        attachment_mode: None,
                        is_dynamic: false,
                        is_attached: true,
                    },
                );
            }
        }

        if let Some(dynamic_groups) = self.dgna_groups.get(&issi) {
            for (&gssi, group) in dynamic_groups {
                groups
                    .entry(gssi)
                    .and_modify(|entry| {
                        entry.is_dynamic = true;
                        entry.mnemonic = group.mnemonic.clone();
                        entry.attachment_mode = Some(group.attachment_mode);
                    })
                    .or_insert_with(|| DeviceGroup {
                        gssi,
                        mnemonic: group.mnemonic.clone(),
                        attachment_mode: Some(group.attachment_mode),
                        is_dynamic: true,
                        is_attached: self
                            .subscribers
                            .get(&issi)
                            .is_some_and(|subscriber| subscriber.attached_groups.contains(&gssi)),
                    });
            }
        }

        groups.into_values().collect()
    }
}

/// Runtime override for the built-in WX/METAR service, edited from the dashboard.
///
/// Mirrors the editable subset of `[wx_service]` config. When `Some`, it takes precedence
/// over the config so toggles/edits apply immediately without a restart; the dashboard
/// also writes the new values back to the TOML so they persist. `None` means "no override
/// — use the config value".
#[derive(Debug, Clone, Default)]
pub struct WxRuntimeOverride {
    pub enabled: bool,
    pub service_issi: u32,
    pub periodic_enabled: bool,
    pub periodic_issi: u32,
    pub periodic_is_group: bool,
    pub periodic_icao: String,
    pub periodic_interval_secs: u64,
}

/// Runtime override for Telegram alerts, edited from the dashboard.
///
/// Mirrors the editable `[telegram_alerts]` config. When `Some`, it takes precedence over the
/// config so toggles/edits (and the detected chat IDs / pasted token) apply immediately without
/// a restart; the dashboard also writes the values back to the TOML so they persist. `None`
/// means "no override — use the config value". The token is kept as a plain `String` here (the
/// state is in-memory only); the config-side `CfgTelegram` wraps it in `SecretField`.
#[derive(Debug, Clone, Default)]
pub struct TelegramRuntimeOverride {
    pub enabled: bool,
    pub bot_token: String,
    pub chat_ids: Vec<i64>,
    pub alert_connect: bool,
    pub alert_disconnect: bool,
    pub alert_t351: bool,
    pub alert_lip: bool,
    pub alert_backhaul: bool,
    pub alert_critical_logs: bool,
}

/// Runtime override for DAPNET receive/send/forwarding settings, edited from the dashboard.
///
/// Mirrors `[dapnet]`. When present, it takes precedence over the config file so routing edits
/// apply immediately; the dashboard also writes the values back to TOML for persistence.
#[derive(Debug, Clone, Default)]
pub struct DapnetRuntimeOverride {
    pub enabled: bool,
    pub api_url: String,
    pub username: String,
    pub password: String,
    pub poll_interval_secs: u64,
    pub forward_sds: bool,
    pub forward_callout: bool,
    pub forward_telegram: bool,
    pub sds_source_issi: u32,
    pub sds_dest_issi: u32,
    pub sds_dest_is_group: bool,
    pub ric_issi_routes: std::collections::BTreeMap<u32, u32>,
    pub ric_gssi_routes: std::collections::BTreeMap<u32, u32>,
    pub sds_allowed_rics: std::collections::BTreeSet<u32>,
    pub callout_allowed_rics: std::collections::BTreeSet<u32>,
    pub telegram_allowed_rics: std::collections::BTreeSet<u32>,
    pub callout_source_issi: u32,
    pub callout_dest_issi: u32,
    pub callout_incident_base: u16,
    pub callout_text_prefix: String,
    pub telegram_prefix: String,
    pub rwth_core_enabled: bool,
    pub rwth_core_host: String,
    pub rwth_core_port: u16,
    pub rwth_core_device: String,
    pub rwth_core_version: String,
    pub rwth_core_callsign: String,
    pub rwth_core_authkey: String,
    pub rwth_messages_limit: usize,
}

/// Runtime override for GeoAlarm settings, edited from the dashboard.
///
/// Mirrors `[geoalarm]`. When present, it takes precedence over the config file so radius,
/// filters and forwarding edits apply immediately; the dashboard also writes the values back to
/// TOML for persistence.
#[derive(Debug, Clone, Default)]
pub struct GeoalarmRuntimeOverride {
    pub enabled: bool,
    pub flowstation_lat: f64,
    pub flowstation_lon: f64,
    pub radius_m: f64,
    pub cooldown_secs: u64,
    pub trigger_tetra: bool,
    pub trigger_meshcom: bool,
    pub forward_tpg2200: bool,
    pub forward_sds: bool,
    pub forward_sip: bool,
    pub forward_telegram: bool,
    pub tetra_issi_whitelist: std::collections::BTreeSet<u32>,
    pub tetra_issi_blacklist: std::collections::BTreeSet<u32>,
    pub meshcom_source_whitelist: std::collections::BTreeSet<String>,
    pub meshcom_source_blacklist: std::collections::BTreeSet<String>,
    pub sds_source_issi: u32,
    pub sds_dest_issi: u32,
    pub sds_dest_is_group: bool,
    pub tpg2200_source_issi: u32,
    pub tpg2200_dest_issi: u32,
    pub tpg2200_incident_base: u16,
    pub tpg2200_text_prefix: String,
    pub tpg2200_max_text_chars: usize,
    pub sip_title_prefix: String,
    pub telegram_prefix: String,
}

/// Runtime override for Snom XML NOTIFY settings, edited from the dashboard.
///
/// Mirrors `[snom_notify]`. When present, it takes precedence over the config file so
/// notification routing edits apply immediately; the dashboard also writes the values
/// back to TOML for persistence.
#[derive(Debug, Clone, Default)]
pub struct SnomNotifyRuntimeOverride {
    pub enabled: bool,
    pub ami_host: String,
    pub ami_port: u16,
    pub ami_username: String,
    pub ami_password: String,
    pub endpoints: Vec<String>,
    pub notify_sds: bool,
    pub notify_dapnet: bool,
    pub notify_telegram: bool,
    pub sds_directions: Vec<String>,
    pub dapnet_allowed_rics: std::collections::BTreeSet<u32>,
    pub sds_allowed_issis: std::collections::BTreeSet<u32>,
    pub title_prefix: String,
    pub notify_event: String,
    pub content_type: String,
    pub subscription_state: String,
    pub max_text_chars: usize,
    pub connect_timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct AsteriskRuntimeStatus {
    pub configured: bool,
    pub enabled: bool,
    pub register_status: String,
    pub sip_listen: String,
    pub remote: String,
    pub rtp_port_range: String,
    pub codec: String,
    pub active_dialogs: usize,
    pub last_rx: Option<String>,
    pub last_tx: Option<String>,
    pub last_error: Option<String>,
}

impl Default for AsteriskRuntimeStatus {
    fn default() -> Self {
        Self {
            configured: false,
            enabled: false,
            register_status: "disabled".to_string(),
            sip_listen: String::new(),
            remote: String::new(),
            rtp_port_range: String::new(),
            codec: "PCMU".to_string(),
            active_dialogs: 0,
            last_rx: None,
            last_tx: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DapnetRuntimeStatus {
    pub configured: bool,
    pub enabled: bool,
    pub rwth_core_enabled: bool,
    pub rwth_core_status: String,
    pub endpoint: String,
    pub callsign: String,
    pub forward_sds: bool,
    pub forward_callout: bool,
    pub forward_telegram: bool,
    pub seen_messages: usize,
    pub last_rx: Option<String>,
    pub last_error: Option<String>,
}

impl Default for DapnetRuntimeStatus {
    fn default() -> Self {
        Self {
            configured: false,
            enabled: false,
            rwth_core_enabled: false,
            rwth_core_status: "disabled".to_string(),
            endpoint: String::new(),
            callsign: String::new(),
            forward_sds: false,
            forward_callout: false,
            forward_telegram: false,
            seen_messages: 0,
            last_rx: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GeoalarmEventStatus {
    pub ts: String,
    pub source: String,
    pub device: String,
    pub lat: f64,
    pub lon: f64,
    pub distance_m: f64,
    pub inside_radius: bool,
    pub alarmed: bool,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GeoalarmRuntimeStatus {
    pub configured: bool,
    pub enabled: bool,
    pub center: String,
    pub radius_m: f64,
    pub trigger_tetra: bool,
    pub trigger_meshcom: bool,
    pub forward_tpg2200: bool,
    pub forward_sds: bool,
    pub forward_sip: bool,
    pub forward_telegram: bool,
    pub seen_positions: u64,
    pub alarm_count: u64,
    pub last_position: Option<String>,
    pub last_alarm: Option<String>,
    pub last_error: Option<String>,
    pub events: Vec<GeoalarmEventStatus>,
}

impl Default for GeoalarmRuntimeStatus {
    fn default() -> Self {
        Self {
            configured: false,
            enabled: false,
            center: String::new(),
            radius_m: 0.0,
            trigger_tetra: false,
            trigger_meshcom: false,
            forward_tpg2200: false,
            forward_sds: false,
            forward_sip: false,
            forward_telegram: false,
            seen_positions: 0,
            alarm_count: 0,
            last_position: None,
            last_alarm: None,
            last_error: None,
            events: Vec::new(),
        }
    }
}

/// Mutable, stack-editable state (mutex-protected).
#[derive(Debug, Clone)]
pub struct StackState {
    pub timeslot_alloc: TimeslotAllocator,
    /// Backhaul/network connection to SwMI (e.g., Brew/TetraPack). False -> fallback mode.
    pub network_connected: bool,
    /// Centralized subscriber registry for local-first routing decisions.
    pub subscribers: SubscriberRegistry,
    /// Queue of live SDS messages injected at runtime via the dashboard.
    /// Transmitted round-robin alongside the static Home Mode Display text.
    pub live_sds_queue: VecDeque<LiveSdsMessage>,
    /// Monotonically incrementing ID counter for live SDS messages.
    pub next_live_sds_id: u32,
    /// Runtime ISSI whitelist override edited from the dashboard. When `Some`, it takes
    /// precedence over the config file's `[security] issi_whitelist` so changes apply
    /// immediately without a restart. An empty Vec here means "open network" (all ISSIs
    /// allowed), exactly like an empty whitelist in config. `None` means "no override —
    /// fall back to the config value". The dashboard also writes the new list back to the
    /// TOML so it survives a restart.
    pub issi_whitelist_override: Option<Vec<u32>>,
    /// Runtime override for the WX/METAR service (dashboard toggle). See WxRuntimeOverride.
    pub wx_override: Option<WxRuntimeOverride>,
    /// Runtime override for Telegram alerts (dashboard editing). See TelegramRuntimeOverride.
    pub telegram_override: Option<TelegramRuntimeOverride>,
    /// Runtime override for DAPNET settings (dashboard editing). See DapnetRuntimeOverride.
    pub dapnet_override: Option<DapnetRuntimeOverride>,
    /// Runtime override for GeoAlarm settings (dashboard editing). See GeoalarmRuntimeOverride.
    pub geoalarm_override: Option<GeoalarmRuntimeOverride>,
    /// Runtime override for Snom XML NOTIFY settings. See SnomNotifyRuntimeOverride.
    pub snom_notify_override: Option<SnomNotifyRuntimeOverride>,
    /// Next TPG2200 ActionURL incident number. Initialised lazily from `[tpg2200_action]`.
    pub tpg2200_action_next_incident: Option<u16>,
    /// Runtime Asterisk SIP/RTP bridge status for `/api/asterisk/status` and the dashboard tab.
    pub asterisk_status: AsteriskRuntimeStatus,
    /// Runtime DAPNET receiver/forwarding status for `/api/dapnet` and the Health tab.
    pub dapnet_status: DapnetRuntimeStatus,
    /// Runtime GeoAlarm status for `/api/geoalarm`.
    pub geoalarm_status: GeoalarmRuntimeStatus,
    /// Live map "identity currently reachable on a traffic channel" → (DL timeslot, usage_marker),
    /// republished every tick by CMCE call control from the live call tables (so it is never
    /// stale). Keyed by GSSI for active group calls and by each participant ISSI for connected
    /// individual calls. The SDS path uses it to steal a FACCH half-slot on the right timeslot
    /// so it can reach an MS engaged in a call, which is NOT listening to the MCCH
    /// (ETSI EN 300 392-2 §23.5). Empty when no calls are active, so idle delivery stays on
    /// the MCCH exactly as before.
    pub active_call_ts: std::collections::HashMap<u32, (u16, u8, u8)>,

    /// Per-MS energy-economy downlink monitoring window, republished every tick by MM from the
    /// live client registry (so it is never stale). Keyed by ISSI; value = (monitoring_frame
    /// 1..=18, monitoring_multiframe, cycle_len). Only MSs granted an actual energy-saving mode
    /// (Eg1..Eg7, cycle_len >= 2) appear here — a StayAlive / unknown MS is ABSENT, which the
    /// scheduler treats as "always reachable" (never gated). Used to defer unsolicited individual
    /// downlink (incoming-call D-SETUP, SDS) until the MS is awake on its window
    /// (ETSI EN 300 392-2 §16.7). Empty when no MS is in energy economy.
    pub ee_monitoring_windows: std::collections::HashMap<u32, (u8, u8, u8)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_deregister() {
        let mut reg = SubscriberRegistry::new();
        assert!(!reg.is_registered(1001));
        reg.register(1001);
        assert!(reg.is_registered(1001));
        reg.deregister(1001);
        assert!(!reg.is_registered(1001));
    }

    #[test]
    fn test_affiliate_deaffiliate() {
        let mut reg = SubscriberRegistry::new();
        reg.register(1001);
        reg.affiliate(1001, 91);
        assert!(reg.has_group_members(91));
        reg.deaffiliate(1001, 91);
        assert!(!reg.has_group_members(91));
    }

    #[test]
    fn test_has_group_members() {
        let mut reg = SubscriberRegistry::new();
        reg.register(1001);
        reg.register(1002);
        reg.register(1003);
        reg.affiliate(1001, 100);
        reg.affiliate(1002, 100);
        reg.affiliate(1003, 100);
        assert!(reg.has_group_members(100));

        // Deaffiliate one, should still have members
        reg.deaffiliate(1001, 100);
        assert!(reg.has_group_members(100));

        // Deregister a user, should still have members
        reg.deregister(1002);
        assert!(reg.has_group_members(100));

        // Deregister last user, should have no members
        reg.deregister(1003);
        assert!(!reg.has_group_members(100));
    }

    #[test]
    fn test_has_group_members_empty() {
        let reg = SubscriberRegistry::new();
        assert!(!reg.has_group_members(999));
    }

    #[test]
    fn test_register_overwrites_existing_subscriber() {
        let mut reg = SubscriberRegistry::new();
        reg.register(1001);
        reg.affiliate(1001, 91);
        assert!(reg.has_group_members(91));

        reg.register(1001);

        assert!(reg.is_registered(1001));
        reg.deaffiliate(1001, 91);
        assert!(!reg.has_group_members(91));
    }

    #[test]
    fn test_all_registered_issis() {
        let mut reg = SubscriberRegistry::new();
        reg.register(1001);
        reg.register(1002);
        reg.register(1003);
        let mut issis: Vec<u32> = reg.all_registered_issis().collect();
        issis.sort_unstable();
        assert_eq!(issis, vec![1001, 1002, 1003]);

        reg.deregister(1002);
        let mut issis: Vec<u32> = reg.all_registered_issis().collect();
        issis.sort_unstable();
        assert_eq!(issis, vec![1001, 1003]);
    }

    #[test]
    fn test_dgna_group_registry_survives_deaffiliate_and_deregister() {
        let mut reg = SubscriberRegistry::new();
        reg.register(1001);
        reg.affiliate(1001, 91);
        reg.remember_dgna_group(1001, 91, Some("OPS".to_string()), 3);

        reg.deaffiliate(1001, 91);
        assert_eq!(
            reg.device_groups_of(1001),
            vec![DeviceGroup {
                gssi: 91,
                mnemonic: Some("OPS".to_string()),
                attachment_mode: Some(3),
                is_dynamic: true,
                is_attached: false,
            }]
        );

        reg.deregister(1001);
        assert!(reg.dgna_groups_of(1001).iter().any(|group| group.gssi == 91));
    }

    #[test]
    fn test_device_groups_merge_static_and_dynamic() {
        let mut reg = SubscriberRegistry::new();
        reg.register(1001);
        reg.affiliate(1001, 91);
        reg.affiliate(1001, 92);
        reg.remember_dgna_group(1001, 92, Some("DYN".to_string()), 1);
        reg.remember_dgna_group(1001, 93, None, 4);

        assert_eq!(
            reg.device_groups_of(1001),
            vec![
                DeviceGroup {
                    gssi: 91,
                    mnemonic: None,
                    attachment_mode: None,
                    is_dynamic: false,
                    is_attached: true,
                },
                DeviceGroup {
                    gssi: 92,
                    mnemonic: Some("DYN".to_string()),
                    attachment_mode: Some(1),
                    is_dynamic: true,
                    is_attached: true,
                },
                DeviceGroup {
                    gssi: 93,
                    mnemonic: None,
                    attachment_mode: Some(4),
                    is_dynamic: true,
                    is_attached: false,
                },
            ]
        );
    }
}

impl Default for StackState {
    fn default() -> Self {
        Self {
            timeslot_alloc: TimeslotAllocator::default(),
            network_connected: false,
            subscribers: SubscriberRegistry::new(),
            live_sds_queue: VecDeque::new(),
            next_live_sds_id: 1,
            issi_whitelist_override: None,
            wx_override: None,
            telegram_override: None,
            dapnet_override: None,
            geoalarm_override: None,
            snom_notify_override: None,
            tpg2200_action_next_incident: None,
            asterisk_status: AsteriskRuntimeStatus::default(),
            dapnet_status: DapnetRuntimeStatus::default(),
            geoalarm_status: GeoalarmRuntimeStatus::default(),
            active_call_ts: std::collections::HashMap::new(),
            ee_monitoring_windows: std::collections::HashMap::new(),
        }
    }
}
