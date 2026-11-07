//! `[networks]`: several TETRA networks served by one station.
//!
//! Each network has its own identity (MCC/MNC) and either a Brew link of its own or none, in
//! which case it is switched inside the station (`local`). Every cell is bound to exactly one
//! network and broadcasts that network's identity, so a radio user who selects a network on the
//! terminal lands on that network's cells and is routed to its core. Omit the section for the
//! classic single-network station: `[net_info]` plus an optional `[brew]` / `[lst_dispatch]`.
//!
//! ```toml
//! network = "tetrapack"            # the primary cell's network (required with [networks])
//!
//! [networks.tetrapack]
//! mcc = 901
//! mnc = 9999
//! [networks.tetrapack.brew]
//! host = "core.tetrapack.online"
//! port = 443
//! tls = true
//! username = 1000001
//! password = "secret"
//!
//! [networks.club]                  # no [brew]: switched locally between its cells
//! mcc = 901
//! mnc = 1
//!
//! [[cells]]
//! id = 1
//! network = "club"
//! ```
//!
//! A network can also be reached *through the Brew server of another network* (`via`, the
//! "Infrastructure" way of Multi Network): its cells broadcast its own MCC/MNC, connect with the
//! other network's `[brew]` login and tell the server, which must offer the `mni` extension, that
//! their radios and groups belong to this network. The server then registers them there (for
//! example on its link to TetraPack). The station holds no credentials of that network.
//!
//! ```toml
//! [networks.tp]
//! mcc = 901
//! mnc = 1638
//! via = "home"                     # the network whose Brew server reaches 901/1638
//! ```

use std::collections::HashMap;

use serde::Deserialize;
use toml::Value;

use super::sec_brew::{CfgBrew, CfgBrewDto, apply_brew_patch};
use super::sec_net::CfgNetInfo;

/// Name of the network a station without a `[networks]` section serves: the one described by
/// `[net_info]` and `[brew]`.
pub const DEFAULT_NETWORK: &str = "default";

/// One network the station serves.
#[derive(Debug, Clone)]
pub struct CfgNetwork {
    /// The `[networks.<name>]` key; what `[[cells]] network = "…"` refers to.
    pub name: String,
    /// MCC/MNC the network's cells broadcast.
    pub net: CfgNetInfo,
    /// The network's core, or `None` for a network switched inside the station only. For a
    /// network reached `via` another one, that network's Brew link.
    pub brew: Option<CfgBrew>,
    /// The network whose Brew server this one is reached through (Multi Network, `mni`).
    pub via: Option<String>,
}

impl CfgNetwork {
    /// True when the network has no core and lives inside the station.
    pub fn is_local(&self) -> bool {
        self.brew.is_none()
    }
}

#[derive(Deserialize)]
struct NetworkDto {
    mcc: Option<u16>,
    mnc: Option<u16>,
    brew: Option<CfgBrewDto>,
    via: Option<String>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

/// Parse the raw `[networks]` table. `default_net` is `[net_info]`, used for a network that
/// gives no `mcc`/`mnc` of its own. Returns the networks in name order, so "the first network"
/// is well defined whatever order the file lists them in.
pub fn parse_networks(raw: Option<Value>, default_net: &CfgNetInfo) -> Result<Vec<CfgNetwork>, String> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let Value::Table(table) = raw else {
        return Err("networks must be a table of named networks ([networks.<name>])".into());
    };
    if table.is_empty() {
        return Err("[networks] is present but names no network".into());
    }
    let mut names: Vec<&String> = table.keys().collect();
    names.sort_unstable();

    let mut networks = Vec::with_capacity(table.len());
    for name in names {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(format!("networks.{name:?}: a network name is letters, digits, '_' or '-'"));
        }
        let dto: NetworkDto = table[name].clone().try_into().map_err(|e| format!("networks.{name}: {e}"))?;
        if !dto.extra.is_empty() {
            let mut keys: Vec<&str> = dto.extra.keys().map(|k| k.as_str()).collect();
            keys.sort_unstable();
            return Err(format!("Unrecognized fields in networks.{name}: {keys:?}"));
        }
        let mcc = dto.mcc.unwrap_or(default_net.mcc);
        let mnc = dto.mnc.unwrap_or(default_net.mnc);
        if mcc > 1023 {
            return Err(format!("networks.{name}.mcc must be 0-1023"));
        }
        if mnc > 16383 {
            return Err(format!("networks.{name}.mnc must be 0-16383"));
        }
        let brew = match dto.brew {
            Some(brew) => {
                if !brew.extra.is_empty() {
                    let mut keys: Vec<&str> = brew.extra.keys().map(|k| k.as_str()).collect();
                    keys.sort_unstable();
                    return Err(format!("Unrecognized fields in networks.{name}.brew: {keys:?}"));
                }
                Some(apply_brew_patch(brew))
            }
            None => None,
        };
        networks.push(CfgNetwork {
            name: name.clone(),
            net: CfgNetInfo { mcc, mnc },
            brew,
            via: dto.via,
        });
    }

    // A network reached through another one's Brew server uses that network's login.
    for i in 0..networks.len() {
        let Some(via) = networks[i].via.clone() else { continue };
        let name = networks[i].name.clone();
        if networks[i].brew.is_some() {
            return Err(format!("networks.{name}: give either brew or via, not both"));
        }
        let Some(target) = networks.iter().find(|n| n.name == via) else {
            return Err(format!("networks.{name}.via: no network {via:?}"));
        };
        if target.via.is_some() || target.brew.is_none() {
            return Err(format!("networks.{name}.via: network {via:?} needs a brew link of its own"));
        }
        networks[i].brew = target.brew.clone();
    }

    // Two networks with one identity would be one network to the radios.
    for (i, a) in networks.iter().enumerate() {
        if let Some(b) = networks[i + 1..].iter().find(|b| b.net.mcc == a.net.mcc && b.net.mnc == a.net.mnc) {
            return Err(format!(
                "networks.{} and networks.{} share MCC/MNC {}/{}: every network needs its own identity",
                a.name, b.name, a.net.mcc, a.net.mnc
            ));
        }
    }
    Ok(networks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bluestation::parsing::from_toml_str;
    use tetra_core::CellId;

    const BASE: &str = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1521
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1
colour_code = 1
"#;

    const TWO_NETWORKS: &str = r#"
[networks.tetrapack]
[networks.tetrapack.brew]
host = "core.example.org"
username = 1000001
password = "secret"
tls = true

[networks.club]
mcc = 901
mnc = 1

[[cells]]
id = 1
network = "club"
[cells.cell_info]
main_carrier = 1525

[[cells]]
id = 2
network = "club"
[cells.cell_info]
main_carrier = 1529
"#;

    fn parse(extra: &str) -> Result<crate::bluestation::StackConfig, String> {
        from_toml_str(&format!("{BASE}{extra}")).map_err(|e| e.to_string())
    }

    /// Root keys must come before the first table, so the primary's binding goes at the top.
    fn parse_primary(network: &str, extra: &str) -> Result<crate::bluestation::StackConfig, String> {
        from_toml_str(&format!("network = {network:?}\n{BASE}{extra}")).map_err(|e| e.to_string())
    }

    #[test]
    fn without_a_networks_section_there_is_one_implicit_network() {
        let cfg = parse("[brew]\nhost = \"core.example.org\"\nusername = 1\npassword = \"x\"\ntls = false\n").unwrap();
        assert!(!cfg.networks_explicit);
        assert_eq!(cfg.network, DEFAULT_NETWORK);
        assert_eq!(cfg.networks.len(), 1);
        let n = &cfg.networks[0];
        assert_eq!((n.name.as_str(), n.net.mcc, n.net.mnc), (DEFAULT_NETWORK, 901, 9999));
        assert!(n.brew.as_ref().is_some_and(|b| b.host == "core.example.org"));
        assert_eq!(cfg.network_of_cell(CellId::PRIMARY).name, DEFAULT_NETWORK);
        assert_eq!(cfg.network_home_cell(DEFAULT_NETWORK), Some(CellId::PRIMARY));
    }

    #[test]
    fn cells_bind_to_named_networks_and_take_their_identity_and_core() {
        let cfg = parse_primary("tetrapack", TWO_NETWORKS).unwrap();
        cfg.validate().unwrap();
        assert!(cfg.networks_explicit);
        assert_eq!(cfg.networks.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), vec!["club", "tetrapack"], "name order");
        // The tetrapack network inherits [net_info] for its identity; club has its own and no core.
        let tp = cfg.network("tetrapack").unwrap();
        assert_eq!((tp.net.mcc, tp.net.mnc), (901, 9999));
        assert!(!tp.is_local());
        let club = cfg.network("club").unwrap();
        assert_eq!((club.net.mcc, club.net.mnc), (901, 1));
        assert!(club.is_local());
        assert_eq!(cfg.network_of_cell(CellId(2)).name, "club");
    }

    #[test]
    fn the_primary_is_bound_at_the_root_and_each_cell_runs_its_network() {
        let cfg = parse_primary("tetrapack", TWO_NETWORKS).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.network, "tetrapack");
        assert_eq!((cfg.net.mcc, cfg.net.mnc), (901, 9999));
        assert!(cfg.brew.as_ref().is_some_and(|b| b.host == "core.example.org"));
        assert_eq!(cfg.network_cells("tetrapack"), vec![CellId(0)]);
        assert_eq!(cfg.network_cells("club"), vec![CellId(1), CellId(2)]);
        assert_eq!(cfg.network_home_cell("club"), Some(CellId(1)));
        // One cell on tetrapack: nothing to switch. Two on club: switched inside the station.
        assert!(!cfg.network_is_linked("tetrapack"));
        assert!(cfg.network_is_linked("club"));
        assert!(!cfg.is_site_linked(), "the primary's own network has one cell");

        let c1 = cfg.for_extra_cell(CellId(1)).unwrap();
        assert_eq!(c1.network, "club");
        assert_eq!((c1.net.mcc, c1.net.mnc), (901, 1));
        assert!(c1.brew.is_none());
        assert!(c1.site_linked);
        assert!(!c1.asterisk.enabled, "Asterisk stays with the primary's network");
        assert_eq!(c1.networks.len(), 2, "a cell still knows every network");
    }

    #[test]
    fn siblings_on_another_network_are_advertised_with_their_identity() {
        let mut cfg = parse_primary("tetrapack", TWO_NETWORKS).unwrap();
        cfg.add_sibling_neighbours();
        let primary_neighbours = &cfg.cell.neighbor_cells_ca;
        assert_eq!(primary_neighbours.len(), 2);
        assert!(primary_neighbours.iter().all(|n| n.mcc == Some(901) && n.mnc == Some(1)), "club cells seen from tetrapack");
        let club = &cfg.extra_cells[0].cell.neighbor_cells_ca;
        let to_primary = club.iter().find(|n| n.main_carrier_number == 1521).unwrap();
        assert_eq!((to_primary.mcc, to_primary.mnc), (Some(901), Some(9999)), "tetrapack seen from club");
        let to_sibling = club.iter().find(|n| n.main_carrier_number == 1529).unwrap();
        assert_eq!((to_sibling.mcc, to_sibling.mnc), (None, None), "same network: no identity needed");
    }

    #[test]
    fn mistakes_are_refused() {
        let err = |extra: &str| parse(extra).unwrap_err();
        assert!(err("[[cells]]\nid = 1\nnetwork = \"x\"\n[cells.cell_info]\nmain_carrier = 1525\n").contains("needs a [networks] section"));
        let err_p = |network: &str, extra: &str| parse_primary(network, extra).unwrap_err();
        assert!(err_p("x", "").contains("needs a [networks.x] section"));
        assert!(err(TWO_NETWORKS).contains("needs `network = "), "the primary's network is not guessed");
        assert!(err_p("tetrapack", &format!("[brew]\nhost = \"h\"\nusername = 1\npassword = \"x\"\ntls = false\n{TWO_NETWORKS}")).contains("[brew] cannot be used together with [networks]"));
        assert!(err_p("nope", TWO_NETWORKS).contains("not a [networks] entry"));
        assert!(err_p("a", "[networks.a]\nmcc = 901\nmnc = 1\n[networks.b]\nmcc = 901\nmnc = 1\n[[cells]]\nid = 1\nnetwork = \"b\"\n[cells.cell_info]\nmain_carrier = 1525\n").contains("share MCC/MNC"));
        assert!(err_p("a", "[networks.a]\nmcc = 901\nmnc = 1\n[networks.b]\nmcc = 901\nmnc = 2\n").contains("no cell serves this network"));
        assert!(err_p("a", "[networks.a]\nmcc = 5000\n").contains("mcc must be 0-1023"));
        assert!(err_p("a", "[networks.a]\nfoo = 1\n").contains("Unrecognized fields in networks.a"));
        assert!(err_p("a", "[networks.a]\n[networks.a.brew]\nhost = \"h\"\nusername = 1\npassword = \"x\"\ntls = false\nfoo = 1\n").contains("Unrecognized fields in networks.a.brew"));
        assert!(err_p("a", "[networks]\n").contains("names no network"));
        assert!(err_p("a", "[networks.\"bad name\"]\nmcc = 901\n").contains("a network name is letters"));
    }

    const VIA: &str = r#"
[networks.home]
[networks.home.brew]
host = "brew.example"
username = 1000001
password = "secret"
tls = false

[networks.tp]
mcc = 901
mnc = 1638
via = "home"

[[cells]]
id = 1
network = "tp"
[cells.cell_info]
main_carrier = 1525
"#;

    #[test]
    fn a_network_via_another_uses_its_brew_link() {
        let cfg = parse_primary("home", VIA).unwrap();
        cfg.validate().unwrap();
        let tp = cfg.network("tp").unwrap();
        assert_eq!(tp.via.as_deref(), Some("home"));
        assert_eq!(tp.brew.as_ref().map(|b| b.host.as_str()), Some("brew.example"));
        assert!(!cfg.network_is_via() && !cfg.mni_enabled());
        let cell = cfg.for_extra_cell(CellId(1)).unwrap();
        assert!(cell.network_is_via() && cell.mni_enabled());
        assert_eq!((cell.net.mcc, cell.net.mnc), (901, 1638));
        assert!(!cell.is_site_linked(), "one cell: its own Brew link, no switch");
        assert_eq!(cfg.cells_of_station_network().len(), 1, "the primary announces only its network's cells");
    }

    #[test]
    fn via_must_name_a_network_with_its_own_brew_link() {
        let both = VIA.replace("via = \"home\"", "via = \"home\"\n[networks.tp.brew]\nhost = \"x\"\nusername = 2\npassword = \"y\"\ntls = false");
        assert!(parse_primary("home", &both).unwrap_err().contains("not both"));
        assert!(parse_primary("home", &VIA.replace("via = \"home\"", "via = \"nowhere\"")).unwrap_err().contains("no network"));
        let no_core = VIA.replace("[networks.home.brew]\nhost = \"brew.example\"\nusername = 1000001\npassword = \"secret\"\ntls = false\n", "");
        assert!(parse_primary("home", &no_core).unwrap_err().contains("brew link of its own"));
    }
}
