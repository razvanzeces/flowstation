//! Radios of other networks registered here by migration (Multi Network).
//!
//! TS 100 392-2 clause 16.4.1.1 case b): the first U-LOCATION UPDATE DEMAND comes on the USSI
//! (the radio's home ISSI, clause 23.4.1.2.5) with its MNI; the SwMI answers with a D-LOCATION
//! UPDATE PROCEEDING that allocates a (V)ASSI, and the radio registers again on that (V)ASSI.
//! The (V)ASSI is valid only in the SwMI that allocated it (clause 16.4.7.1) and is the identity
//! the rest of the stack (CMCE, dashboard) uses for the visitor. Groups of the visitor's home
//! network get a (V)GSSI (clause 16.8.7).
//!
//! The ranges both identities come from are configuration (`[multi_network]`), not ETSI. Each
//! cell of a station allocates only the values whose offset in the range equals its cell id
//! modulo 8, so two cells never hand out the same (V)ASSI.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tetra_config::bluestation::{CfgMultiNetwork, Mni};

/// How long a (V)ASSI handed out in a D-LOCATION UPDATE PROCEEDING waits for the second
/// U-LOCATION UPDATE DEMAND before it may be reused. Project choice (not from ETSI).
pub const PENDING_TTL: Duration = Duration::from_secs(60);

/// Values per allocation stride: one per possible cell id (`CellId::MAX` + 1).
const STRIDE: u32 = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct Visitor {
    pub mni: Mni,
    /// The radio's own ISSI in its home network.
    pub issi: u32,
    /// The second U-LOCATION UPDATE DEMAND was accepted.
    pub registered: bool,
    pub since: Instant,
}

/// How a visitor named a group in the group identity uplink (clause 16.8.7), so the downlink
/// answer can use the form the clause asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GroupForm {
    /// GSSI + address extension of its home network (GIAT 1).
    Home { mni: Mni, gssi: u32 },
    /// A (V)GSSI allocated earlier (GIAT 2).
    Vgssi,
}

#[derive(Debug, Default)]
pub struct Visitors {
    cell: u32,
    by_vassi: HashMap<u32, Visitor>,
    vgssi_of: HashMap<(Mni, u32), u32>,
    home_of: HashMap<u32, (Mni, u32)>,
}

impl Visitors {
    pub fn new(cell: u8) -> Self {
        Visitors {
            cell: u32::from(cell) % STRIDE,
            ..Default::default()
        }
    }

    pub fn get(&self, vassi: u32) -> Option<&Visitor> {
        self.by_vassi.get(&vassi)
    }

    pub fn is_visitor(&self, ssi: u32) -> bool {
        self.by_vassi.contains_key(&ssi)
    }

    pub fn len(&self) -> usize {
        self.by_vassi.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_vassi.is_empty()
    }

    /// The (V)ASSI currently held by the radio `issi` of network `mni`.
    pub fn vassi_of(&self, mni: Mni, issi: u32) -> Option<u32> {
        self.by_vassi
            .iter()
            .find(|(_, v)| v.mni == mni && v.issi == issi)
            .map(|(vassi, _)| *vassi)
    }

    /// A new (V)ASSI for the radio `issi` of network `mni` (clause 16.4.7.1: a migration makes the
    /// previous one invalid and the SwMI allocates a new one). `in_use` names values this station
    /// already uses for something else. Returns the old (V)ASSI too, if the radio had one.
    pub fn allocate(
        &mut self,
        cfg: &CfgMultiNetwork,
        mni: Mni,
        issi: u32,
        now: Instant,
        in_use: impl Fn(u32) -> bool,
    ) -> Option<(u32, Option<u32>)> {
        let previous = self.vassi_of(mni, issi);
        if let Some(old) = previous {
            self.by_vassi.remove(&old);
        }
        self.by_vassi
            .retain(|_, v| v.registered || now.duration_since(v.since) < PENDING_TTL);
        let taken = &self.by_vassi;
        let vassi = Self::free_value(cfg.vassi_range, self.cell, |v| taken.contains_key(&v) || in_use(v))?;
        self.by_vassi.insert(
            vassi,
            Visitor {
                mni,
                issi,
                registered: false,
                since: now,
            },
        );
        Some((vassi, previous))
    }

    pub fn confirm(&mut self, vassi: u32) {
        if let Some(v) = self.by_vassi.get_mut(&vassi) {
            v.registered = true;
        }
    }

    pub fn remove(&mut self, vassi: u32) -> Option<Visitor> {
        self.by_vassi.remove(&vassi)
    }

    /// The (V)GSSI of group `gssi` of network `mni`, allocated on first use.
    pub fn vgssi_for(&mut self, cfg: &CfgMultiNetwork, mni: Mni, gssi: u32, in_use: impl Fn(u32) -> bool) -> Option<u32> {
        if let Some(v) = self.vgssi_of.get(&(mni, gssi)) {
            return Some(*v);
        }
        let taken = &self.home_of;
        let vgssi = Self::free_value(cfg.vgssi_range, self.cell, |v| taken.contains_key(&v) || in_use(v))?;
        self.vgssi_of.insert((mni, gssi), vgssi);
        self.home_of.insert(vgssi, (mni, gssi));
        Some(vgssi)
    }

    /// The (V)GSSI already given to group `gssi` of network `mni`.
    pub fn known_vgssi(&self, mni: Mni, gssi: u32) -> Option<u32> {
        self.vgssi_of.get(&(mni, gssi)).copied()
    }

    /// The home group (network, GSSI) behind a (V)GSSI.
    pub fn home_group(&self, vgssi: u32) -> Option<(Mni, u32)> {
        self.home_of.get(&vgssi).copied()
    }

    fn free_value(range: (u32, u32), cell: u32, used: impl Fn(u32) -> bool) -> Option<u32> {
        let (first, last) = range;
        let mut v = first.checked_add(cell)?;
        while v <= last {
            if !used(v) {
                return Some(v);
            }
            v = v.checked_add(STRIDE)?;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CfgMultiNetwork {
        CfgMultiNetwork {
            enabled: true,
            networks: vec![Mni { mcc: 901, mnc: 1234 }],
            vassi_range: (1000, 1063),
            vgssi_range: (2000, 2063),
        }
    }

    const HOME: Mni = Mni { mcc: 901, mnc: 1234 };

    #[test]
    fn cells_never_share_a_vassi() {
        let now = Instant::now();
        let mut a = Visitors::new(0);
        let mut b = Visitors::new(1);
        let (va, _) = a.allocate(&cfg(), HOME, 2320972, now, |_| false).unwrap();
        let (vb, _) = b.allocate(&cfg(), HOME, 2320973, now, |_| false).unwrap();
        assert_eq!(va, 1000);
        assert_eq!(vb, 1001);
        let (va2, _) = a.allocate(&cfg(), HOME, 2320974, now, |_| false).unwrap();
        assert_eq!(va2, 1008, "next value of cell 0 is one stride on");
    }

    #[test]
    fn a_new_migration_replaces_the_old_vassi_and_skips_used_values() {
        let now = Instant::now();
        let mut v = Visitors::new(0);
        let (first, prev) = v.allocate(&cfg(), HOME, 2320972, now, |s| s == 1000).unwrap();
        assert_eq!((first, prev), (1008, None));
        let (second, prev) = v.allocate(&cfg(), HOME, 2320972, now, |_| false).unwrap();
        assert_eq!(prev, Some(1008));
        assert_eq!(v.vassi_of(HOME, 2320972), Some(second));
        assert_eq!(v.len(), 1, "only one alias per ITSI (clause 16.4.7.0)");
    }

    #[test]
    fn unconfirmed_vassis_expire_and_the_range_can_run_out() {
        let now = Instant::now();
        let mut v = Visitors::new(7);
        for issi in 0..8 {
            assert!(v.allocate(&cfg(), HOME, issi, now, |_| false).is_some());
        }
        assert!(v.allocate(&cfg(), HOME, 99, now, |_| false).is_none(), "8 values per cell in 1000..=1063");
        let later = now + PENDING_TTL + Duration::from_secs(1);
        assert!(v.allocate(&cfg(), HOME, 99, later, |_| false).is_some(), "pending ones lapse");
    }

    #[test]
    fn a_home_group_keeps_its_vgssi() {
        let mut v = Visitors::new(0);
        let a = v.vgssi_for(&cfg(), HOME, 9, |_| false).unwrap();
        assert_eq!(v.vgssi_for(&cfg(), HOME, 9, |_| false), Some(a));
        let other = Mni { mcc: 232, mnc: 1 };
        assert_ne!(v.vgssi_for(&cfg(), other, 9, |_| false), Some(a), "same GSSI, other network");
        assert_eq!(v.home_group(a), Some((HOME, 9)));
    }
}
