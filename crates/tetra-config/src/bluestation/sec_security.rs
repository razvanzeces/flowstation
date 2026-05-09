use serde::Deserialize;

/// Access control / security configuration
#[derive(Debug, Clone, Default)]
pub struct CfgSecurity {
    /// ISSI whitelist. If non-empty, only these ISSIs are allowed to register.
    /// An empty list means all ISSIs are accepted (open network).
    /// Example config:
    ///   [security]
    ///   issi_whitelist = [2260571, 1001, 1002]
    pub issi_whitelist: Vec<u32>,
}

impl CfgSecurity {
    /// Returns true if the given ISSI is allowed to register.
    /// When the whitelist is empty, all ISSIs are allowed.
    pub fn is_issi_allowed(&self, issi: u32) -> bool {
        if self.issi_whitelist.is_empty() {
            return true;
        }
        self.issi_whitelist.contains(&issi)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CfgSecurityDto {
    #[serde(default)]
    pub issi_whitelist: Vec<u32>,
}

pub fn apply_security_patch(dto: CfgSecurityDto) -> CfgSecurity {
    CfgSecurity {
        issi_whitelist: dto.issi_whitelist,
    }
}
