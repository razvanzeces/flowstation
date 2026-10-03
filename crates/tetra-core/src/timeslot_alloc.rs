#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeslotOwner {
    Brew,
    Cmce,
    /// A packet-data channel (PDCH) of the SNDCP bearer: on the main carrier, or on the
    /// packet-data carrier; any other owner takes it when nothing else is free (voice first).
    PacketData,
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
    /// Packet-data slots another owner took, until the packet-data bearer drains them.
    preempted: Vec<CarrierSlot>,
    /// A carrier whose slots only packet data may take (`[packet_data] pdch_carrier_exclusive`).
    data_only_carrier: Option<u16>,
}

impl Default for TimeslotAllocator {
    fn default() -> Self {
        Self {
            carriers: vec![0],
            owners: vec![[None, None, None, None]],
            preempted: Vec::new(),
            data_only_carrier: None,
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

    /// Only packet data may take the slots of `carrier` (None: every carrier takes voice).
    pub fn set_data_only_carrier(&mut self, carrier: Option<u16>) {
        self.data_only_carrier = carrier;
    }

    /// Whether the carrier at `carrier_i` is the data-only one.
    fn data_only(&self, carrier_i: usize) -> bool {
        self.data_only_carrier == Some(self.carriers[carrier_i])
    }

    pub fn allocate_any(&mut self, owner: TimeslotOwner) -> Option<u8> {
        self.allocate_any_slot(owner).map(|slot| slot.ts)
    }

    pub fn allocate_any_slot(&mut self, owner: TimeslotOwner) -> Option<CarrierSlot> {
        let data_only = self.data_only_carrier;
        for (carrier_i, slots) in self.owners.iter_mut().enumerate() {
            if owner != TimeslotOwner::PacketData && data_only == Some(self.carriers[carrier_i]) {
                continue;
            }
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
        // Nothing free on any carrier: a packet-data channel gives way.
        if owner != TimeslotOwner::PacketData {
            for ts in 2..=4u8 {
                let slot = &mut self.owners[0][ts as usize - 1];
                if *slot == Some(TimeslotOwner::PacketData) {
                    *slot = Some(owner);
                    let taken = CarrierSlot {
                        carrier_num: self.carriers[0],
                        ts,
                    };
                    self.preempted.push(taken);
                    return Some(taken);
                }
            }
            // Then a packet-data channel of the packet-data carrier (never a data-only one).
            for carrier_i in 1..self.carriers.len() {
                if self.data_only(carrier_i) {
                    continue;
                }
                for ts in 1..=4u8 {
                    let slot = &mut self.owners[carrier_i][ts as usize - 1];
                    if *slot == Some(TimeslotOwner::PacketData) {
                        *slot = Some(owner);
                        let taken = CarrierSlot {
                            carrier_num: self.carriers[carrier_i],
                            ts,
                        };
                        self.preempted.push(taken);
                        return Some(taken);
                    }
                }
            }
        }
        None
    }

    /// A packet-data channel on the main carrier: the first free timeslot of `prefs` (2..=4 only).
    pub fn reserve_packet_data_slot(&mut self, prefs: &[u8]) -> Option<CarrierSlot> {
        let ts = prefs
            .iter()
            .copied()
            .find(|ts| (2..=4).contains(ts) && self.owners[0][*ts as usize - 1].is_none())?;
        self.owners[0][ts as usize - 1] = Some(TimeslotOwner::PacketData);
        Some(CarrierSlot {
            carrier_num: self.carriers[0],
            ts,
        })
    }

    /// A packet-data channel of up to `n` main-carrier timeslots: the first free ones of `prefs`
    /// (2..=4), in that order.
    pub fn reserve_packet_data_slots(&mut self, prefs: &[u8], n: usize) -> Vec<CarrierSlot> {
        (0..n).map_while(|_| self.reserve_packet_data_slot(prefs)).collect()
    }

    /// A packet-data channel of up to `n` slots of `carrier`: the first free ones of `prefs`
    /// (2..=4 on the main carrier, 1..=4 on any other), in that order.
    pub fn reserve_packet_data_slots_on(&mut self, carrier: u16, prefs: &[u8], n: usize) -> Vec<CarrierSlot> {
        let Ok(carrier_i) = self.carrier_idx(carrier) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &ts in prefs {
            if out.len() >= n {
                break;
            }
            if self.slot_supported_for_carrier(carrier_i, ts) && self.owners[carrier_i][ts as usize - 1].is_none() {
                self.owners[carrier_i][ts as usize - 1] = Some(TimeslotOwner::PacketData);
                out.push(CarrierSlot { carrier_num: carrier, ts });
            }
        }
        out
    }

    /// Traffic slots without an owner: (main carrier ts2..=4, every other carrier ts1..=4 but a
    /// data-only one). For the voice headroom of a packet-data channel of several slots.
    pub fn free_traffic_slots(&self) -> (usize, usize) {
        let main = self.owners[0][1..].iter().filter(|owner| owner.is_none()).count();
        let others = (1..self.carriers.len())
            .filter(|i| !self.data_only(*i))
            .flat_map(|i| self.owners[i].iter())
            .filter(|owner| owner.is_none())
            .count();
        (main, others)
    }

    /// Packet-data slots taken by another owner since the last call.
    pub fn drain_preempted_packet_data(&mut self) -> Vec<CarrierSlot> {
        std::mem::take(&mut self.preempted)
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

    /// Number of traffic timeslots on the primary configured carrier a call can get: the
    /// unallocated ones and the packet-data ones, which give way.
    pub fn free_count(&self) -> usize {
        self.owners[0][1..].iter().filter(|owner| Self::yields(owner)).count()
    }

    fn yields(owner: &Option<TimeslotOwner>) -> bool {
        matches!(owner, None | Some(TimeslotOwner::PacketData))
    }

    /// Traffic slots a call can get on any carrier (a data-only carrier is never one).
    pub fn free_slot_count(&self) -> usize {
        self.owners
            .iter()
            .enumerate()
            .filter(|(carrier_i, _)| !self.data_only(*carrier_i))
            .flat_map(|(carrier_i, slots)| {
                let slice = if carrier_i == 0 { &slots[1..] } else { &slots[..] };
                slice.iter()
            })
            .filter(|owner| Self::yields(owner))
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

    /// Voice takes a packet-data slot only when nothing else is free on any carrier, and the
    /// packet-data bearer learns it once.
    #[test]
    fn voice_takes_the_packet_data_slot_last() {
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584, 1585]);
        let pdch = alloc.reserve_packet_data_slot(&[4, 3, 2]).expect("packet-data slot");
        assert_eq!(pdch, CarrierSlot { carrier_num: 1584, ts: 4 });
        assert_eq!(alloc.free_slot_count(), 7, "the packet-data slot counts as free");
        assert_eq!(alloc.free_count(), 3);
        let mut voice = Vec::new();
        for _ in 0..6 {
            voice.push(alloc.allocate_any_slot(TimeslotOwner::Cmce).expect("voice slot"));
        }
        assert!(!voice.contains(&pdch), "main ts2-3 and the secondary carrier first: {voice:?}");
        assert!(alloc.drain_preempted_packet_data().is_empty());
        assert_eq!(alloc.slot_owner(pdch), Some(TimeslotOwner::PacketData));
        assert_eq!(alloc.free_slot_count(), 1);
        assert_eq!(
            alloc.allocate_any_slot(TimeslotOwner::Brew),
            Some(pdch),
            "then the packet-data slot"
        );
        assert_eq!(alloc.slot_owner(pdch), Some(TimeslotOwner::Brew));
        assert_eq!(alloc.drain_preempted_packet_data(), vec![pdch]);
        assert!(alloc.drain_preempted_packet_data().is_empty(), "reported once");
        assert_eq!(alloc.allocate_any_slot(TimeslotOwner::Cmce), None);
        assert_eq!(alloc.free_slot_count(), 0);
    }

    #[test]
    fn packet_data_slot_only_on_the_main_carrier_ts2_to_4() {
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584, 1585]);
        assert_eq!(alloc.reserve_packet_data_slot(&[1, 5, 0]), None, "never ts1 or out of range");
        alloc.reserve(TimeslotOwner::Cmce, 3).unwrap();
        let first = alloc.reserve_packet_data_slot(&[3, 2]).expect("the next preferred slot");
        assert_eq!(first, CarrierSlot { carrier_num: 1584, ts: 2 });
        let second = alloc.reserve_packet_data_slot(&[2, 3, 4]).expect("ts4");
        assert_eq!(second.ts, 4);
        assert_eq!(
            alloc.reserve_packet_data_slot(&[2, 3, 4]),
            None,
            "main carrier full, secondary never used"
        );
        assert_eq!(alloc.free_slot_count(), 6, "two packet-data slots and the four secondary ones");
    }

    #[test]
    fn packet_data_slots_follow_the_preferences() {
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584]);
        let slots = alloc.reserve_packet_data_slots(&[4, 3, 2], 2);
        assert_eq!(slots.iter().map(|s| s.ts).collect::<Vec<_>>(), vec![4, 3]);
        assert!(slots.iter().all(|s| s.carrier_num == 1584));

        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584]);
        alloc.reserve(TimeslotOwner::Cmce, 3).unwrap();
        let slots = alloc.reserve_packet_data_slots(&[4, 3, 2], 2);
        assert_eq!(slots.iter().map(|s| s.ts).collect::<Vec<_>>(), vec![4, 2], "not contiguous");
        assert!(alloc.reserve_packet_data_slots(&[4, 3, 2], 3).is_empty(), "nothing left");

        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584]);
        alloc.reserve(TimeslotOwner::Cmce, 2).unwrap();
        alloc.reserve(TimeslotOwner::Cmce, 3).unwrap();
        assert_eq!(alloc.reserve_packet_data_slots(&[4, 3, 2], 3).len(), 1, "what is free");

        let (mut a, mut b) = (TimeslotAllocator::default(), TimeslotAllocator::default());
        a.configure_carriers(&[1584]);
        b.configure_carriers(&[1584]);
        assert_eq!(
            a.reserve_packet_data_slots(&[3, 4], 1),
            vec![b.reserve_packet_data_slot(&[3, 4]).unwrap()]
        );
    }

    #[test]
    fn free_traffic_slots_counts_main_and_others() {
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584]);
        alloc.reserve(TimeslotOwner::Cmce, 2).unwrap();
        assert_eq!(alloc.free_traffic_slots(), (2, 0));
        alloc.reserve_packet_data_slot(&[4]).unwrap();
        assert_eq!(alloc.free_traffic_slots(), (1, 0), "a packet-data slot has an owner");

        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584, 1585]);
        assert_eq!(alloc.free_traffic_slots(), (3, 4));
    }

    #[test]
    fn releasing_a_packet_data_slot_voice_took_is_refused() {
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584]);
        let pdch = alloc.reserve_packet_data_slot(&[2]).unwrap();
        alloc.reserve(TimeslotOwner::Cmce, 3).unwrap();
        alloc.reserve(TimeslotOwner::Cmce, 4).unwrap();
        assert_eq!(alloc.allocate_any_slot(TimeslotOwner::Cmce), Some(pdch));
        assert_eq!(
            alloc.release_slot(TimeslotOwner::PacketData, pdch),
            Err(TimeslotAllocErr::OwnerMismatch {
                carrier_num: 1584,
                ts: 2,
                owner: TimeslotOwner::PacketData,
                actual: TimeslotOwner::Cmce,
            })
        );
        alloc.release_slot(TimeslotOwner::Cmce, pdch).unwrap();
        assert!(alloc.slot_is_free(pdch));
    }

    #[test]
    fn packet_data_slots_on_another_carrier_may_take_ts1() {
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584, 1585]);
        let slots = alloc.reserve_packet_data_slots_on(1585, &[4, 3, 2, 1], 4);
        assert_eq!(slots.iter().map(|s| s.ts).collect::<Vec<_>>(), vec![4, 3, 2, 1]);
        assert!(slots.iter().all(|s| s.carrier_num == 1585));
        assert!(
            alloc.reserve_packet_data_slots_on(1584, &[1], 1).is_empty(),
            "never ts1 of the main carrier"
        );
        assert!(alloc.reserve_packet_data_slots_on(1599, &[2], 1).is_empty(), "no such carrier");
        assert_eq!(alloc.free_traffic_slots(), (3, 0));
    }

    #[test]
    fn voice_preempts_a_carrier_packet_data_slot_last() {
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584, 1585]);
        for ts in 2..=4 {
            alloc.reserve(TimeslotOwner::Cmce, ts).unwrap();
        }
        assert_eq!(alloc.reserve_packet_data_slots_on(1585, &[1, 2, 3, 4], 4).len(), 4);
        let taken = alloc.allocate_any_slot(TimeslotOwner::Cmce).expect("a carrier packet-data slot");
        assert_eq!(taken, CarrierSlot { carrier_num: 1585, ts: 1 });
        assert_eq!(alloc.drain_preempted_packet_data(), vec![taken]);
        assert!(alloc.drain_preempted_packet_data().is_empty(), "reported once");

        // A main-carrier packet-data slot goes first.
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584, 1585]);
        alloc.reserve(TimeslotOwner::Cmce, 2).unwrap();
        alloc.reserve(TimeslotOwner::Cmce, 3).unwrap();
        let main = alloc.reserve_packet_data_slot(&[4]).unwrap();
        assert_eq!(alloc.reserve_packet_data_slots_on(1585, &[1, 2, 3, 4], 4).len(), 4);
        assert_eq!(alloc.allocate_any_slot(TimeslotOwner::Cmce), Some(main));
        assert_eq!(
            alloc.allocate_any_slot(TimeslotOwner::Cmce),
            Some(CarrierSlot { carrier_num: 1585, ts: 1 })
        );
    }

    #[test]
    fn a_data_only_carrier_is_never_given_to_voice() {
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584, 1585]);
        alloc.set_data_only_carrier(Some(1585));
        assert_eq!(alloc.free_slot_count(), 3);
        assert_eq!(alloc.free_traffic_slots(), (3, 0));
        let voice: Vec<_> = (0..3).map(|_| alloc.allocate_any_slot(TimeslotOwner::Cmce).unwrap()).collect();
        assert!(voice.iter().all(|s| s.carrier_num == 1584), "{voice:?}");
        assert_eq!(alloc.allocate_any_slot(TimeslotOwner::Cmce), None);
        assert_eq!(alloc.free_slot_count(), 0);
        assert_eq!(
            alloc.reserve_packet_data_slots_on(1585, &[4, 3, 2, 1], 4).len(),
            4,
            "data still goes there"
        );
        assert_eq!(alloc.allocate_any_slot(TimeslotOwner::Brew), None, "and is never preempted");
        alloc.configure_carriers(&[1584, 1585]);
        assert_eq!(alloc.free_slot_count(), 0, "kept across a carrier reconfiguration");
    }

    /// Three carriers: 11 voice slots, main ts2-4 first, then every slot of the others in order; a
    /// data-only extra carrier is skipped.
    #[test]
    fn three_carriers_give_eleven_voice_slots_in_carrier_order() {
        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584, 1585, 1587]);
        assert_eq!(alloc.free_slot_count(), 11);
        assert_eq!(alloc.free_traffic_slots(), (3, 8));
        let voice: Vec<(u16, u8)> = (0..11)
            .map(|_| alloc.allocate_any_slot(TimeslotOwner::Cmce).expect("voice slot"))
            .map(|s| (s.carrier_num, s.ts))
            .collect();
        let expected: Vec<(u16, u8)> = (2..=4)
            .map(|ts| (1584, ts))
            .chain((1..=4).map(|ts| (1585, ts)))
            .chain((1..=4).map(|ts| (1587, ts)))
            .collect();
        assert_eq!(voice, expected);
        assert_eq!(alloc.allocate_any_slot(TimeslotOwner::Cmce), None);

        let mut alloc = TimeslotAllocator::default();
        alloc.configure_carriers(&[1584, 1585, 1587]);
        alloc.set_data_only_carrier(Some(1587));
        assert_eq!(alloc.free_slot_count(), 7);
        let voice: Vec<_> = (0..7).map(|_| alloc.allocate_any_slot(TimeslotOwner::Cmce).unwrap()).collect();
        assert!(voice.iter().all(|s| s.carrier_num != 1587), "{voice:?}");
        assert_eq!(alloc.allocate_any_slot(TimeslotOwner::Cmce), None);
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
