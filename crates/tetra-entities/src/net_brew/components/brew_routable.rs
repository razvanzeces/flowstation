use tetra_config::bluestation::{CfgBrew, SharedConfig};
use tetra_core::tetra_entities::TetraEntity;

pub const BREW_ENTITIES: [TetraEntity; 2] = [TetraEntity::Brew, TetraEntity::Brew2];

#[inline]
pub fn is_brew_entity(entity: TetraEntity) -> bool {
    matches!(entity, TetraEntity::Brew | TetraEntity::Brew2)
}

#[inline]
pub fn brew_config_for_entity(config: &SharedConfig, entity: TetraEntity) -> Option<CfgBrew> {
    let cfg = config.config();
    match entity {
        TetraEntity::Brew => cfg.brew.clone(),
        TetraEntity::Brew2 => cfg.brew2.clone(),
        _ => None,
    }
}

/// Returns true if the Brew component is active
#[inline]
pub fn is_active(config: &SharedConfig) -> bool {
    config.config().brew.is_some() || config.config().brew2.is_some()
}

#[inline]
pub fn is_active_for_entity(config: &SharedConfig, entity: TetraEntity) -> bool {
    brew_config_for_entity(config, entity).is_some()
}

/// Returns true if the SDS over Brew feature is enabled
#[inline]
pub fn feature_sds_enabled(config: &SharedConfig) -> bool {
    BREW_ENTITIES
        .iter()
        .copied()
        .any(|entity| feature_sds_enabled_for_entity(config, entity))
}

#[inline]
pub fn feature_sds_enabled_for_entity(config: &SharedConfig, entity: TetraEntity) -> bool {
    brew_config_for_entity(config, entity).is_some_and(|brew| brew.feature_sds_enabled)
}

/// Returns true if the configured Brew server is TetraPack (core.tetrapack.online)
fn is_tetrapack_server(brew_config: &CfgBrew) -> bool {
    brew_config.host == "core.tetrapack.online"
}

fn is_pbx_gateway_issi(brew_config: &CfgBrew, issi: u32) -> bool {
    brew_config
        .pbx_gateway_issis
        .as_ref()
        .is_some_and(|allowed| allowed.contains(&issi))
}

#[inline]
pub fn is_brew_local_issi_allowed_for_entity(config: &SharedConfig, entity: TetraEntity, issi: u32) -> bool {
    brew_config_for_entity(config, entity).is_some_and(|brew| brew.local_issi_allowed(issi))
}

/// Pick the one Brew entity that may represent this local TETRA ISSI.
///
/// Returning `None` on ambiguity is deliberate: a local terminal must never be registered or
/// forwarded through two Brew backhauls at the same time.
pub fn route_entity_for_local_issi(config: &SharedConfig, issi: u32) -> Option<TetraEntity> {
    let mut routed = None;
    for entity in BREW_ENTITIES {
        if is_brew_local_issi_allowed_for_entity(config, entity, issi) {
            if routed.is_some() {
                return None;
            }
            routed = Some(entity);
        }
    }
    routed
}

/// Determine if a given GSSI should be routed over Brew, or is restricted to local handling
pub fn is_brew_gssi_routable(config: &SharedConfig, ssi: u32) -> bool {
    BREW_ENTITIES
        .iter()
        .copied()
        .any(|entity| is_brew_gssi_routable_for_entity(config, entity, ssi))
}

pub fn is_brew_gssi_routable_for_entity(config: &SharedConfig, entity: TetraEntity, ssi: u32) -> bool {
    let Some(brew_config) = brew_config_for_entity(config, entity) else {
        return false;
    };
    if config.config().cell.local_ssi_ranges.contains(ssi) {
        // Range overridden as local
        return false;
    }

    // Check if whitelist is present and if so, check
    if let Some(whitelist) = &brew_config.whitelisted_ssis {
        if whitelist.contains(&ssi) {
            // Range explicitly whitelisted for routing to Brew
            return true;
        } else {
            // Not in whitelist - block routing to Brew
            return false;
        }
    }

    // No whitelist present, default to allow
    true
}

/// Determine whether a Brew-originated INBOUND call/SDS for a given GSSI may be admitted locally.
///
/// This is deliberately weaker than [`is_brew_gssi_routable`]. That predicate governs OUTBOUND
/// forwarding of *local* traffic to Brew and therefore honours `whitelisted_ssis` — which is
/// documented as "allow only calls for selected SSIs to be **forwarded through Brew**", i.e. an
/// outbound concept. Applying the whitelist to inbound admission wrongly blocks a bridging/foreign
/// GSSI that is absent from the whitelist (FH-FEAT-032 R3): a network call legitimately arriving
/// from an authenticated Brew connection must still reach the local MS camped on that group.
///
/// The `local_ssi_ranges` override is still honoured — those ranges are documented as local-only
/// ("Incoming brew traffic on these ranges will also be rejected"), so inbound traffic to them stays
/// rejected.
#[inline]
pub fn is_brew_inbound_allowed(config: &SharedConfig, ssi: u32) -> bool {
    is_active(config) && !config.config().cell.local_ssi_ranges.contains(ssi)
}

#[inline]
pub fn is_brew_inbound_allowed_for_entity(config: &SharedConfig, entity: TetraEntity, ssi: u32) -> bool {
    is_active_for_entity(config, entity) && !config.config().cell.local_ssi_ranges.contains(ssi)
}

/// Determine whether Brew-originated external subscriber state may be mirrored into CMCE.
///
/// Subscriber events for SSI ranges or explicit local Brew assignments are looped-back state, not
/// external listeners. They must not be represented a second time through either Brew connection.
#[inline]
pub fn is_brew_external_subscriber_allowed_for_entity(config: &SharedConfig, entity: TetraEntity, issi: u32) -> bool {
    if brew_config_for_entity(config, entity).is_none() || config.config().cell.local_ssi_ranges.contains(issi) {
        return false;
    }

    !BREW_ENTITIES.into_iter().any(|candidate| {
        brew_config_for_entity(config, candidate).is_some_and(|brew| brew.has_local_issi_allowlist() && brew.local_issi_allowed(issi))
    })
}

/// Determine if a given ISSI should be sent to the Brew server.
/// On TetraPack, subscriber ISSIs must be 7 digits (1_000_000..=9_999_999).
/// Special service ISSIs (e.g. 600 echo, short numbers) are always forwarded to Brew —
/// TetraPack Core handles them internally; blocking them here causes "Service Denied".
pub fn is_brew_issi_routable(config: &SharedConfig, issi: u32) -> bool {
    BREW_ENTITIES
        .iter()
        .copied()
        .any(|entity| is_brew_issi_routable_for_entity(config, entity, issi))
}

pub fn is_brew_issi_routable_for_entity(config: &SharedConfig, entity: TetraEntity, issi: u32) -> bool {
    let Some(brew_config) = brew_config_for_entity(config, entity) else {
        return false;
    };

    if is_tetrapack_server(&brew_config) {
        // 7-digit subscriber ISSIs are always routable.
        // Short ISSIs (< 1_000_000) are service numbers handled by TetraPack Core —
        // let them through so the core can respond (echo test 600, etc.)
        (issi >= 1_000_000 && issi <= 9_999_999) || issi < 1_000_000 || is_pbx_gateway_issi(&brew_config, issi)
    } else {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_config::bluestation::{SharedConfig, parsing::from_toml_str};

    fn shared_config(extra_cell: &str) -> SharedConfig {
        let toml = format!(
            r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1584
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1
{}

[brew]
host = "example.invalid"
port = 443
tls = true
username = 0
password = ""
"#,
            extra_cell
        );
        SharedConfig::from_parts(from_toml_str(&toml).expect("test config parses"), None)
    }

    #[test]
    fn external_subscriber_state_respects_local_ssi_ranges() {
        let cfg = shared_config("local_ssi_ranges = [[999, 999], [9998, 9999]]");

        assert!(!is_brew_external_subscriber_allowed_for_entity(&cfg, TetraEntity::Brew, 999));
        assert!(!is_brew_external_subscriber_allowed_for_entity(&cfg, TetraEntity::Brew, 9999));
        assert!(is_brew_external_subscriber_allowed_for_entity(&cfg, TetraEntity::Brew, 2632585));
    }

    #[test]
    fn external_subscriber_state_respects_explicit_local_issi_allowlists() {
        let toml = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1584
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1

[brew]
host = "example.invalid"
port = 443
tls = true
username = 0
password = ""
local_issi_allowlist = [2632585, 2632586]
local_issi_blocklist = [2632586]

[brew2]
host = "example2.invalid"
port = 443
tls = true
username = 0
password = ""
local_issi_allowlist = [2633869]
"#;
        let cfg = SharedConfig::from_parts(from_toml_str(toml).expect("test config parses"), None);

        for entity in BREW_ENTITIES {
            assert!(!is_brew_external_subscriber_allowed_for_entity(&cfg, entity, 2632585));
            assert!(!is_brew_external_subscriber_allowed_for_entity(&cfg, entity, 2633869));
            assert!(is_brew_external_subscriber_allowed_for_entity(&cfg, entity, 2147004));
        }
        assert_eq!(route_entity_for_local_issi(&cfg, 2632585), Some(TetraEntity::Brew));
        assert_eq!(route_entity_for_local_issi(&cfg, 2633869), Some(TetraEntity::Brew2));
        assert_eq!(route_entity_for_local_issi(&cfg, 2632586), None, "blocklist must win");
        assert_eq!(route_entity_for_local_issi(&cfg, 2147004), None, "unassigned ISSI must stay local");
    }
}
