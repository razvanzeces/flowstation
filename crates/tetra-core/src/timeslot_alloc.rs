#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeslotOwner {
    Brew,
    Cmce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CarrierSlot {
    pub carrier_num: u16,
    pub ts: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeslotAllocErr {
    InvalidTimeslot(u8),
    InvalidCarrier(u16),
    InUse {
        carrier_num: u16,
        ts: u8,
        owner: TimeslotOwner,
    },
    NotAllocated {
        carrier_num: u16,
        ts: u8,
    },
    OwnerMismatch {
        carrier_num: u16,
        ts: u8,
        owner: TimeslotOwner,
        actual: TimeslotOwner,
    },
}

#[derive(Debug, Clone)]
pub struct TimeslotAllocator {
    carriers: Vec<u16>,
    // One [TS1, TS2, TS3, TS4] owner row per configured carrier.
    // The primary carrier keeps TS1 reserved for MCCH/common control; any
    // configured secondary carrier may allocate TS1 for assigned traffic.
    owners: Vec<[Option<TimeslotOwner>; 4]>,
}

impl Default for TimeslotAllocator {
    fn default() -> Self {
        Self {
            carriers: vec![0],
            owners: vec![[None, None, None, None]],
        }
    }
}

impl TimeslotAllocator {
    fn carrier_idx(&self, carrier_num: u16) -> Result<usize, TimeslotAllocErr> {
        self.carriers
            .iter()
            .position(|carrier| *carrier == carrier_num)
            .ok_or(TimeslotAllocErr::InvalidCarrier(carrier_num))
    }

    fn slot_supported_for_carrier(&self, carrier_idx: usize, ts: u8) -> bool {
        match carrier_idx {
            0 => (2..=4).contains(&ts),
            _ => (1..=4).contains(&ts),
        }
    }

    fn slot_idx(&self, carrier_idx: usize, ts: u8) -> Result<usize, TimeslotAllocErr> {
        if self.slot_supported_for_carrier(carrier_idx, ts) {
            Ok((ts - 1) as usize)
        } else {
            Err(TimeslotAllocErr::InvalidTimeslot(ts))
        }
    }

    pub fn configure_carriers(&mut self, carriers: &[u16]) {
        let mut new_carriers = Vec::with_capacity(carriers.len().max(1));
        for carrier in carriers {
            if !new_carriers.contains(carrier) {
                new_carriers.push(*carrier);
            }
        }
        if new_carriers.is_empty() {
            new_carriers.push(0);
        }

        let mut new_owners = vec![[None, None, None, None]; new_carriers.len()];
        for (old_i, old_carrier) in self.carriers.iter().enumerate() {
            if let Some(new_i) = new_carriers.iter().position(|carrier| carrier == old_carrier) {
                new_owners[new_i] = self.owners[old_i];
            }
        }

        self.carriers = new_carriers;
        self.owners = new_owners;
    }

    pub fn carriers(&self) -> &[u16] {
        &self.carriers
    }

    pub fn allocate_any(&mut self, owner: TimeslotOwner) -> Option<u8> {
        self.allocate_any_slot(owner).map(|slot| slot.ts)
    }

    pub fn allocate_any_slot(&mut self, owner: TimeslotOwner) -> Option<CarrierSlot> {
        for (carrier_i, slots) in self.owners.iter_mut().enumerate() {
            let ts_range = if carrier_i == 0 { 2..=4 } else { 1..=4 };
            for ts in ts_range {
                let slot = &mut slots[ts as usize - 1];
                if slot.is_none() {
                    *slot = Some(owner);
                    return Some(CarrierSlot {
                        carrier_num: self.carriers[carrier_i],
                        ts,
                    });
                }
            }
        }
        None
    }

    pub fn reserve(&mut self, owner: TimeslotOwner, ts: u8) -> Result<(), TimeslotAllocErr> {
        let carrier_num = self.carriers[0];
        self.reserve_slot(owner, CarrierSlot { carrier_num, ts })
    }

    pub fn reserve_slot(&mut self, owner: TimeslotOwner, slot: CarrierSlot) -> Result<(), TimeslotAllocErr> {
        let carrier_idx = self.carrier_idx(slot.carrier_num)?;
        let idx = self.slot_idx(carrier_idx, slot.ts)?;
        match self.owners[carrier_idx][idx] {
            None => {
                self.owners[carrier_idx][idx] = Some(owner);
                Ok(())
            }
            Some(existing) => Err(TimeslotAllocErr::InUse {
                carrier_num: slot.carrier_num,
                ts: slot.ts,
                owner: existing,
            }),
        }
    }

    pub fn release(&mut self, owner: TimeslotOwner, ts: u8) -> Result<(), TimeslotAllocErr> {
        let carrier_num = self.carriers[0];
        self.release_slot(owner, CarrierSlot { carrier_num, ts })
    }

    pub fn release_slot(&mut self, owner: TimeslotOwner, slot: CarrierSlot) -> Result<(), TimeslotAllocErr> {
        let carrier_idx = self.carrier_idx(slot.carrier_num)?;
        let idx = self.slot_idx(carrier_idx, slot.ts)?;
        match self.owners[carrier_idx][idx] {
            None => Err(TimeslotAllocErr::NotAllocated {
                carrier_num: slot.carrier_num,
                ts: slot.ts,
            }),
            Some(existing) if existing != owner => Err(TimeslotAllocErr::OwnerMismatch {
                carrier_num: slot.carrier_num,
                ts: slot.ts,
                owner,
                actual: existing,
            }),
            Some(_) => {
                self.owners[carrier_idx][idx] = None;
                Ok(())
            }
        }
    }

    pub fn owner(&self, ts: u8) -> Option<TimeslotOwner> {
        self.slot_owner(CarrierSlot {
            carrier_num: self.carriers[0],
            ts,
        })
    }

    pub fn slot_owner(&self, slot: CarrierSlot) -> Option<TimeslotOwner> {
        let carrier_idx = self.carrier_idx(slot.carrier_num).ok()?;
        let idx = self.slot_idx(carrier_idx, slot.ts).ok()?;
        self.owners[carrier_idx][idx]
    }

    pub fn is_free(&self, ts: u8) -> bool {
        self.owner(ts).is_none()
    }

    pub fn slot_is_free(&self, slot: CarrierSlot) -> bool {
        self.slot_owner(slot).is_none()
    }

    /// Number of currently unallocated traffic timeslots on the primary configured carrier.
    pub fn free_count(&self) -> usize {
        self.owners[0][1..].iter().filter(|owner| owner.is_none()).count()
    }

    pub fn free_slot_count(&self) -> usize {
        self.owners
            .iter()
            .enumerate()
            .flat_map(|(carrier_i, slots)| {
                let slice = if carrier_i == 0 { &slots[1..] } else { &slots[..] };
                slice.iter()
            })
            .filter(|owner| owner.is_none())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_carrier_compatibility_defaults_to_first_carrier() {
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584]);

        let ts = alloc.allocate_any(TimeslotOwner::Cmce).expect("slot");
        assert_eq!(ts, 2);
        assert_eq!(alloc.slot_owner(CarrierSlot { carrier_num: 1584, ts }), Some(TimeslotOwner::Cmce));
        assert_eq!(alloc.owner(ts), Some(TimeslotOwner::Cmce));
    }

    #[test]
    fn dual_carrier_allows_same_timeslot_once_per_carrier() {
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584, 1585]);

        alloc
            .reserve_slot(TimeslotOwner::Cmce, CarrierSlot { carrier_num: 1584, ts: 2 })
            .expect("main carrier reserve");
        alloc
            .reserve_slot(TimeslotOwner::Brew, CarrierSlot { carrier_num: 1585, ts: 2 })
            .expect("secondary carrier reserve");

        assert_eq!(alloc.free_slot_count(), 5);
    }

    #[test]
    fn secondary_ts1_is_allocatable_but_primary_ts1_is_not() {
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584, 1585]);

        assert_eq!(
            alloc.reserve_slot(TimeslotOwner::Cmce, CarrierSlot { carrier_num: 1584, ts: 1 }),
            Err(TimeslotAllocErr::InvalidTimeslot(1))
        );

        alloc
            .reserve_slot(TimeslotOwner::Cmce, CarrierSlot { carrier_num: 1585, ts: 1 })
            .expect("secondary TS1 reserve");
        assert_eq!(
            alloc.slot_owner(CarrierSlot { carrier_num: 1585, ts: 1 }),
            Some(TimeslotOwner::Cmce)
        );
    }

    #[test]
    fn configure_carriers_preserves_existing_owner_state() {
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584, 1585]);
        alloc
            .reserve_slot(TimeslotOwner::Cmce, CarrierSlot { carrier_num: 1585, ts: 3 })
            .expect("reserve");

        alloc.configure_carriers(&[1585, 1586, 1585]);

        assert_eq!(alloc.carriers(), &[1585, 1586]);
        assert_eq!(
            alloc.slot_owner(CarrierSlot { carrier_num: 1585, ts: 3 }),
            Some(TimeslotOwner::Cmce)
        );
    }
}
