use std::collections::HashMap;

use serde::Deserialize;
use toml::Value;

/// A TETRA network, named by its MNI: MCC (10 bits) and MNC (14 bits), TS 100 392-2 clause
/// 18.4.2.1 table 18.17.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub struct Mni {
    pub mcc: u16,
    pub mnc: u16,
}

impl Mni {
    /// The 24-bit address extension (MCC then MNC, clause 16.10.1).
    pub fn address_extension(&self) -> u32 {
        ((self.mcc as u32 & 0x3FF) << 14) | (self.mnc as u32 & 0x3FFF)
    }

    pub fn from_address_extension(ext: u64) -> Self {
        Mni {
            mcc: ((ext >> 14) & 0x3FF) as u16,
            mnc: (ext & 0x3FFF) as u16,
        }
    }
}

impl std::fmt::Display for Mni {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.mcc, self.mnc)
    }
}

/// Multi Network: radios of other networks registering here by migration (TS 100 392-2 clause
/// 16.4.1.1 case b)). Off by default; off means every migrating location update is rejected with
/// "Migration not supported" as before.
#[derive(Debug, Clone)]
pub struct CfgMultiNetwork {
    pub enabled: bool,
    /// Home networks whose radios may migrate here. A radio of any other network is rejected.
    pub networks: Vec<Mni>,
    /// (V)ASSIs given to visiting radios (clause 16.4.7.1), first..=last. Must not hold an ISSI
    /// of this network. Project choice, not from ETSI.
    pub vassi_range: (u32, u32),
    /// (V)GSSIs given to groups of a visitor's home network (clause 16.8.7), first..=last.
    pub vgssi_range: (u32, u32),
}

pub const DEFAULT_VASSI_RANGE: (u32, u32) = (0xF0_0000, 0xF0_FFFF);
pub const DEFAULT_VGSSI_RANGE: (u32, u32) = (0xF1_0000, 0xF1_FFFF);

impl Default for CfgMultiNetwork {
    fn default() -> Self {
        CfgMultiNetwork {
            enabled: false,
            networks: Vec::new(),
            vassi_range: DEFAULT_VASSI_RANGE,
            vgssi_range: DEFAULT_VGSSI_RANGE,
        }
    }
}

impl CfgMultiNetwork {
    pub fn is_vassi(&self, ssi: u32) -> bool {
        self.enabled && (self.vassi_range.0..=self.vassi_range.1).contains(&ssi)
    }

    pub fn is_vgssi(&self, ssi: u32) -> bool {
        self.enabled && (self.vgssi_range.0..=self.vgssi_range.1).contains(&ssi)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        for (first, last) in [self.vassi_range, self.vgssi_range] {
            if first == 0 || last > 0xFF_FFFF || first > last {
                return Err("multi_network ranges must be 1..=16777215 with first <= last");
            }
            // One block per cell id (0..=7), see the allocator in MM.
            if last - first < 7 {
                return Err("multi_network ranges must hold at least 8 values");
            }
        }
        let (a, b) = (self.vassi_range, self.vgssi_range);
        if a.0 <= b.1 && b.0 <= a.1 {
            return Err("multi_network.vassi_range and vgssi_range must not overlap");
        }
        for n in &self.networks {
            if n.mcc > 0x3FF || n.mnc > 0x3FFF {
                return Err("multi_network.networks: mcc is 10 bits, mnc 14 bits");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CfgMultiNetworkDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub networks: Vec<Mni>,
    #[serde(default)]
    pub vassi_range: Option<(u32, u32)>,
    #[serde(default)]
    pub vgssi_range: Option<(u32, u32)>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

pub fn apply_multi_network_patch(dto: CfgMultiNetworkDto) -> CfgMultiNetwork {
    CfgMultiNetwork {
        enabled: dto.enabled,
        networks: dto.networks,
        vassi_range: dto.vassi_range.unwrap_or(DEFAULT_VASSI_RANGE),
        vgssi_range: dto.vgssi_range.unwrap_or(DEFAULT_VGSSI_RANGE),
    }
}

/// A network the Brew server can reach (`SERVICE_NETWORKS`, Brew extension `mni`).
#[derive(Debug, Clone, PartialEq)]
pub struct BrewNetwork {
    pub mni: Mni,
    pub name: String,
    pub up: bool,
}

/// Multi Network state shared by MM (writes) and the Brew worker (reads): who the visitors are,
/// which home groups the (V)GSSIs stand for, and what the server last said about networks.
#[derive(Debug, Clone, Default)]
pub struct MultiNetworkState {
    /// (V)ASSI -> (home network, home ISSI), registered visitors only.
    pub visitors: std::collections::HashMap<u32, (Mni, u32)>,
    /// (V)GSSI -> (home network, home GSSI).
    pub groups: std::collections::HashMap<u32, (Mni, u32)>,
    /// The server's own network and the networks it reaches, from the last `SERVICE_NETWORKS`
    /// of this connection (`None`: none yet, or the server does not offer `mni`).
    pub server: Option<(Mni, Vec<BrewNetwork>)>,
}

impl MultiNetworkState {
    pub fn vassi_of(&self, mni: Mni, issi: u32) -> Option<u32> {
        self.visitors.iter().find(|(_, v)| **v == (mni, issi)).map(|(k, _)| *k)
    }

    pub fn vgssi_of(&self, mni: Mni, gssi: u32) -> Option<u32> {
        self.groups.iter().find(|(_, v)| **v == (mni, gssi)).map(|(k, _)| *k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_extension_round_trip() {
        let mni = Mni { mcc: 901, mnc: 9999 };
        assert_eq!(mni.address_extension(), (901 << 14) | 9999);
        assert_eq!(Mni::from_address_extension(mni.address_extension() as u64), mni);
    }

    #[test]
    fn defaults_are_off_and_valid() {
        let c = apply_multi_network_patch(CfgMultiNetworkDto::default());
        assert!(!c.enabled);
        assert!(c.validate().is_ok());
        assert!(!c.is_vassi(DEFAULT_VASSI_RANGE.0), "nothing is a (V)ASSI while off");
    }

    #[test]
    fn parses_networks_and_rejects_overlap() {
        let dto: CfgMultiNetworkDto = toml::from_str(
            "enabled = true\nnetworks = [{ mcc = 901, mnc = 1234 }]\nvassi_range = [100, 200]\nvgssi_range = [150, 300]",
        )
        .unwrap();
        let c = apply_multi_network_patch(dto);
        assert_eq!(c.networks, vec![Mni { mcc: 901, mnc: 1234 }]);
        assert!(c.validate().is_err());
    }
}
