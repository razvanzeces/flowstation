use serde::Deserialize;
use std::collections::HashMap;
use toml::Value;

#[derive(Debug, Clone)]
pub struct CfgNetInfo {
    /// 10 bits, from 18.4.2.1 D-MLE-SYNC
    pub mcc: u16,
    /// 14 bits, from 18.4.2.1 D-MLE-SYNC
    pub mnc: u16,
}

#[derive(Default, Deserialize)]
pub struct NetInfoDto {
    pub mcc: u16,
    pub mnc: u16,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

pub fn net_dto_to_cfg(ni: NetInfoDto) -> CfgNetInfo {
    CfgNetInfo { mcc: ni.mcc, mnc: ni.mnc }
}
