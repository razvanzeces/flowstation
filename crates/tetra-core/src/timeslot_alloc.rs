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
    // One [TS2, TS3, TS4] owner row per configured carrier.
    owners: Vec<[Option<TimeslotOwner>; 3]>,
}

impl Default for TimeslotAllocator {
    fn default() -> Self {
        Self {
            carriers: vec![0],
            owners: vec![[None, None, None]],
        }
    }
}

impl TimeslotAllocator {
    fn idx(ts: u8) -> Result<usize, TimeslotAllocErr> {
        if (2..=4).contains(&ts) {
            Ok((ts - 2) as usize)
        } else {
            Err(TimeslotAllocErr::InvalidTimeslot(ts))
        }
    }

    fn carrier_idx(&self, carrier_num: u16) -> Result<usize, TimeslotAllocErr> {
        self.carriers
            .iter()
            .position(|carrier| *carrier == carrier_num)
            .ok_or(TimeslotAllocErr::InvalidCarrier(carrier_num))
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

        let mut new_owners = vec![[None, None, None]; new_carriers.len()];
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
            for (slot_i, slot) in slots.iter_mut().enumerate() {
                if slot.is_none() {
                    *slot = Some(owner);
                    return Some(CarrierSlot {
                        carrier_num: self.carriers[carrier_i],
                        ts: slot_i as u8 + 2,
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
        let idx = Self::idx(slot.ts)?;
        let carrier_idx = self.carrier_idx(slot.carrier_num)?;
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
        let idx = Self::idx(slot.ts)?;
        let carrier_idx = self.carrier_idx(slot.carrier_num)?;
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
        let idx = Self::idx(slot.ts).ok()?;
        let carrier_idx = self.carrier_idx(slot.carrier_num).ok()?;
        self.owners[carrier_idx][idx]
    }

    pub fn is_free(&self, ts: u8) -> bool {
        self.owner(ts).is_none()
    }

    pub fn slot_is_free(&self, slot: CarrierSlot) -> bool {
        self.slot_owner(slot).is_none()
    }

    /// Number of currently unallocated traffic timeslots (TS2..=TS4) on the primary configured carrier.
    pub fn free_count(&self) -> usize {
        self.owners[0].iter().filter(|owner| owner.is_none()).count()
    }

    pub fn free_slot_count(&self) -> usize {
        self.owners
            .iter()
            .flat_map(|slots| slots.iter())
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

        assert_eq!(alloc.free_slot_count(), 4);
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
