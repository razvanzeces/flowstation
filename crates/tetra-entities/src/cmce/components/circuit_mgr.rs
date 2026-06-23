use std::collections::{HashMap, VecDeque};

use tetra_core::{CarrierSlot, Direction, TdmaTime, TimeslotAllocator, TimeslotOwner, frames, multiframes};
use tetra_pdus::cmce::structs::cmce_circuit::CmceCircuit;
use tetra_saps::{
    control::enums::{circuit_mode_type::CircuitModeType, communication_type::CommunicationType},
    lcmc::CallId,
};

const D_SETUP_REPEATS: i32 = 1;
const LATE_ENTRY_INTERVAL_TIMESLOTS: i32 = multiframes!(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitErr {
    NoCircuitFree,
    CircuitAlreadyInUse,
    CircuitNotActive,
}

pub enum CircuitMgrCmd {
    SendDSetup(CallId, u8, u16, u8), // call id, usage number, carrier, timeslot
    SendClose(CallId, CmceCircuit),
}

pub struct CircuitMgr {
    pub dltime: TdmaTime,

    /// Holds any Dl and Dl+Ul circuits.
    pub dl: Vec<CmceCircuit>,
    /// Holds any Ul-only circuits, with no recipients on this cell.
    pub ul_only: Vec<CmceCircuit>,

    /// Data blocks queued to be transmitted, per carrier/timeslot.
    pub tx_data: HashMap<(u16, u8), VecDeque<Vec<u8>>>,

    /// 14-bit call identifier. Zero value is reserved.
    pub next_call_identifier: u16,
    /// 5-bit usage number. Values 0-3 are reserved.
    pub next_usage_number: u8,
}

impl CircuitMgr {
    pub fn new() -> Self {
        Self {
            dltime: TdmaTime::default(),
            dl: Vec::new(),
            ul_only: Vec::new(),
            tx_data: HashMap::new(),
            next_call_identifier: 4,
            next_usage_number: 4,
        }
    }

    fn key(carrier_num: u16, ts: u8) -> (u16, u8) {
        (carrier_num, ts)
    }

    fn find_dl(&self, carrier_num: u16, ts: u8) -> Option<&CmceCircuit> {
        self.dl.iter().find(|c| c.carrier_num == carrier_num && c.ts == ts)
    }

    fn find_ul_only(&self, carrier_num: u16, ts: u8) -> Option<&CmceCircuit> {
        self.ul_only.iter().find(|c| c.carrier_num == carrier_num && c.ts == ts)
    }

    fn remove_dl(&mut self, carrier_num: u16, ts: u8) -> Option<CmceCircuit> {
        let index = self.dl.iter().position(|c| c.carrier_num == carrier_num && c.ts == ts)?;
        Some(self.dl.remove(index))
    }

    fn remove_ul_only(&mut self, carrier_num: u16, ts: u8) -> Option<CmceCircuit> {
        let index = self.ul_only.iter().position(|c| c.carrier_num == carrier_num && c.ts == ts)?;
        Some(self.ul_only.remove(index))
    }

    /// Checks if a circuit is active on the given carrier/timeslot.
    /// Returns (dl_active, ul_active).
    pub fn is_active_slot(&self, carrier_num: u16, ts: u8) -> (bool, bool) {
        match self.find_dl(carrier_num, ts) {
            Some(dl) => {
                if dl.direction == Direction::Both {
                    (true, true)
                } else {
                    (true, self.find_ul_only(carrier_num, ts).is_some())
                }
            }
            None => (false, self.find_ul_only(carrier_num, ts).is_some()),
        }
    }

    /// Backwards-compatible check by timeslot; true if any configured carrier uses it.
    pub fn is_active(&self, ts: u8) -> (bool, bool) {
        let dl_active = self.dl.iter().any(|c| c.ts == ts);
        let ul_active = self.ul_only.iter().any(|c| c.ts == ts) || self.dl.iter().any(|c| c.ts == ts && c.direction == Direction::Both);
        (dl_active, ul_active)
    }

    pub fn is_active_dir_slot(&self, carrier_num: u16, ts: u8, dir: Direction) -> bool {
        match dir {
            Direction::Dl => self.find_dl(carrier_num, ts).is_some(),
            Direction::Ul => {
                self.find_ul_only(carrier_num, ts).is_some()
                    || self.find_dl(carrier_num, ts).is_some_and(|dl| dl.direction == Direction::Both)
            }
            _ => {
                tracing::error!(
                    "CMCE: is_active_dir_slot called with non-specific direction {:?}, returning false",
                    dir
                );
                false
            }
        }
    }

    pub fn is_active_dir(&self, ts: u8, dir: Direction) -> bool {
        match dir {
            Direction::Dl => self.dl.iter().any(|c| c.ts == ts),
            Direction::Ul => {
                self.ul_only.iter().any(|c| c.ts == ts) || self.dl.iter().any(|c| c.ts == ts && c.direction == Direction::Both)
            }
            _ => {
                tracing::error!("CMCE: is_active_dir called with non-specific direction {:?}, returning false", dir);
                false
            }
        }
    }

    pub fn get_usage_slot(&self, carrier_num: u16, ts: u8) -> (Option<u8>, Option<u8>) {
        let (dl_usage, dl_is_both) = if let Some(dl) = self.find_dl(carrier_num, ts) {
            (Some(dl.usage), dl.direction == Direction::Both)
        } else {
            (None, false)
        };
        let ul_usage = if dl_is_both {
            dl_usage
        } else {
            self.find_ul_only(carrier_num, ts).map(|ul| ul.usage)
        };
        (dl_usage, ul_usage)
    }

    pub fn get_usage(&self, ts: u8) -> (Option<u8>, Option<u8>) {
        let circuit = self
            .dl
            .iter()
            .find(|c| c.ts == ts)
            .or_else(|| self.ul_only.iter().find(|c| c.ts == ts));
        circuit.map_or((None, None), |circuit| self.get_usage_slot(circuit.carrier_num, ts))
    }

    pub fn get_next_call_id(&mut self) -> CallId {
        let call_id = self.next_call_identifier;
        self.next_call_identifier += 1;
        if self.next_call_identifier > 0x3FF {
            self.next_call_identifier = 1;
        }
        call_id
    }

    pub fn get_next_usage_number(&mut self) -> u8 {
        let usage = self.next_usage_number;
        self.next_usage_number += 1;
        if self.next_usage_number > 63 {
            self.next_usage_number = 4;
        }
        usage
    }

    fn make_circuit(
        &mut self,
        dir: Direction,
        slot: CarrierSlot,
        call_id: CallId,
        usage: u8,
        comm_type: CommunicationType,
        simplex_duplex: bool,
    ) -> CmceCircuit {
        CmceCircuit {
            ts_created: self.dltime,
            direction: dir,
            ts: slot.ts,
            carrier_num: slot.carrier_num,
            call_id,
            usage,
            circuit_mode: CircuitModeType::TchS,
            comm_type,
            simplex_duplex,
            speech_service: Some(0),
            etee_encrypted: false,
        }
    }

    fn get_free_slot(&self, dir: Direction) -> Result<CarrierSlot, CircuitErr> {
        for carrier_num in self
            .dl
            .iter()
            .map(|c| c.carrier_num)
            .chain(self.ul_only.iter().map(|c| c.carrier_num))
            .chain([0])
        {
            for ts in 2..=4 {
                let (dl_active, ul_active) = self.is_active_slot(carrier_num, ts);
                match (dir, dl_active, ul_active) {
                    (Direction::Dl, false, _) => return Ok(CarrierSlot { carrier_num, ts }),
                    (Direction::Ul, false, false) => return Ok(CarrierSlot { carrier_num, ts }),
                    (Direction::Ul, true, false) => {
                        let dl = self.find_dl(carrier_num, ts).unwrap();
                        if dl.direction != Direction::Both {
                            return Ok(CarrierSlot { carrier_num, ts });
                        }
                    }
                    (Direction::Both, false, false) => return Ok(CarrierSlot { carrier_num, ts }),
                    _ => {}
                }
            }
        }
        Err(CircuitErr::NoCircuitFree)
    }

    pub fn allocate_circuit(
        &mut self,
        dir: Direction,
        comm_type: CommunicationType,
        simplex_duplex: bool,
    ) -> Result<&CmceCircuit, CircuitErr> {
        let slot = self.get_free_slot(dir)?;
        let call_id = self.get_next_call_id();
        let usage = self.get_next_usage_number();
        let circuit = self.make_circuit(dir, slot, call_id, usage, comm_type, simplex_duplex);
        self.open_circuit(dir, circuit)
    }

    pub fn allocate_circuit_with_allocator(
        &mut self,
        dir: Direction,
        comm_type: CommunicationType,
        simplex_duplex: bool,
        timeslot_alloc: &mut TimeslotAllocator,
        owner: TimeslotOwner,
    ) -> Result<&CmceCircuit, CircuitErr> {
        let slot = timeslot_alloc.allocate_any_slot(owner).ok_or(CircuitErr::NoCircuitFree)?;
        let call_id = self.get_next_call_id();
        let usage = self.get_next_usage_number();
        let circuit = self.make_circuit(dir, slot, call_id, usage, comm_type, simplex_duplex);
        self.open_circuit(dir, circuit)
    }

    pub fn allocate_circuit_for_call_with_allocator(
        &mut self,
        call_id: CallId,
        dir: Direction,
        comm_type: CommunicationType,
        simplex_duplex: bool,
        timeslot_alloc: &mut TimeslotAllocator,
        owner: TimeslotOwner,
    ) -> Result<&CmceCircuit, CircuitErr> {
        let slot = timeslot_alloc.allocate_any_slot(owner).ok_or(CircuitErr::NoCircuitFree)?;
        let usage = self.get_next_usage_number();
        let circuit = self.make_circuit(dir, slot, call_id, usage, comm_type, simplex_duplex);
        self.open_circuit(dir, circuit)
    }

    pub fn close_circuit(&mut self, dir: Direction, ts: u8) -> Result<CmceCircuit, CircuitErr> {
        let carrier_num = self
            .dl
            .iter()
            .chain(self.ul_only.iter())
            .find(|c| c.ts == ts)
            .map(|c| c.carrier_num)
            .ok_or(CircuitErr::CircuitNotActive)?;
        self.close_circuit_slot(dir, carrier_num, ts)
    }

    pub fn close_circuit_slot(&mut self, dir: Direction, carrier_num: u16, ts: u8) -> Result<CmceCircuit, CircuitErr> {
        match dir {
            Direction::Dl | Direction::Both => {
                self.tx_data.remove(&Self::key(carrier_num, ts));
                let circuit = self.remove_dl(carrier_num, ts);
                circuit.ok_or(CircuitErr::CircuitNotActive)
            }
            Direction::Ul => self.remove_ul_only(carrier_num, ts).ok_or(CircuitErr::CircuitNotActive),
            _ => panic!(),
        }
    }

    fn open_circuit(&mut self, dir: Direction, circuit: CmceCircuit) -> Result<&CmceCircuit, CircuitErr> {
        let carrier_num = circuit.carrier_num;
        let ts = circuit.ts;
        let (dl_active, ul_active) = self.is_active_slot(carrier_num, ts);
        if dir.includes_dl() && dl_active {
            return Err(CircuitErr::CircuitAlreadyInUse);
        }
        if dir.includes_ul() && ul_active {
            return Err(CircuitErr::CircuitAlreadyInUse);
        }

        match dir {
            Direction::Dl | Direction::Both => {
                let key = Self::key(carrier_num, ts);
                if self.tx_data.get(&key).is_some_and(|queue| !queue.is_empty()) {
                    tracing::warn!("CircuitMgr::create had pending tx_data on Dl carrier={} ts={}", carrier_num, ts);
                    self.tx_data.remove(&key);
                }
                self.dl.push(circuit);
                Ok(self.dl.last().unwrap())
            }
            Direction::Ul => {
                self.ul_only.push(circuit);
                Ok(self.ul_only.last().unwrap())
            }
            _ => panic!(),
        }
    }

    pub fn put_block(&mut self, carrier_num: u16, ts: u8, block: Vec<u8>) -> Result<(), CircuitErr> {
        if !self.is_active_dir_slot(carrier_num, ts, Direction::Dl) {
            Err(CircuitErr::CircuitNotActive)
        } else {
            self.tx_data.entry(Self::key(carrier_num, ts)).or_default().push_back(block);
            Ok(())
        }
    }

    pub fn take_block(&mut self, carrier_num: u16, ts: u8) -> Result<Option<Vec<u8>>, CircuitErr> {
        if !self.is_active_dir_slot(carrier_num, ts, Direction::Dl) {
            return Err(CircuitErr::CircuitNotActive);
        }
        Ok(self.tx_data.get_mut(&Self::key(carrier_num, ts)).and_then(VecDeque::pop_front))
    }

    /// Closes any circuits that have expired.
    /// Full-duplex circuits are exempt; only simplex circuits are force-closed here.
    fn close_expired_circuits(&mut self, mut tasks: Option<Vec<CircuitMgrCmd>>) -> Option<Vec<CircuitMgrCmd>> {
        const CIRCUIT_EXPIRY_TIMESLOTS: i32 = 6 * 60 * 18 * 4;

        let mut to_close = self
            .dl
            .iter()
            .filter(|circuit| !circuit.simplex_duplex)
            .filter(|circuit| circuit.ts_created.age(self.dltime) > CIRCUIT_EXPIRY_TIMESLOTS)
            .map(|circuit| (circuit.direction, circuit.carrier_num, circuit.ts, circuit.call_id))
            .collect::<Vec<_>>();
        to_close.extend(
            self.ul_only
                .iter()
                .filter(|circuit| !circuit.simplex_duplex)
                .filter(|circuit| circuit.ts_created.age(self.dltime) > CIRCUIT_EXPIRY_TIMESLOTS)
                .map(|circuit| (circuit.direction, circuit.carrier_num, circuit.ts, circuit.call_id)),
        );
        for (dir, carrier_num, ts, call_id) in to_close {
            match self.close_circuit_slot(dir, carrier_num, ts) {
                Ok(circuit) => tasks.get_or_insert_with(Vec::new).push(CircuitMgrCmd::SendClose(call_id, circuit)),
                Err(_) => {
                    tracing::debug!(
                        "circuit_mgr: expiry close skipped for call_id={} carrier={} ts={} dir={:?} (already closed)",
                        call_id,
                        carrier_num,
                        ts,
                        dir
                    );
                }
            }
        }
        tasks
    }

    pub fn tick_start(&mut self, dltime: TdmaTime) -> Option<Vec<CircuitMgrCmd>> {
        self.dltime = dltime;
        let mut tasks = None;

        if dltime.t == 1 {
            tasks = self.close_expired_circuits(tasks);

            for circuit in self.dl.iter() {
                let age = circuit.ts_created.age(dltime);
                if age < frames!(D_SETUP_REPEATS) || (age / 4) % (LATE_ENTRY_INTERVAL_TIMESLOTS / 4) == 0 {
                    tasks.get_or_insert_with(Vec::new).push(CircuitMgrCmd::SendDSetup(
                        circuit.call_id,
                        circuit.usage,
                        circuit.carrier_num,
                        circuit.ts,
                    ));
                }
            }
            return tasks;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplex_circuit_not_closed_after_six_minutes() {
        let mut mgr = CircuitMgr::new();
        mgr.dl.push(CmceCircuit {
            ts_created: TdmaTime { h: 0, m: 1, f: 1, t: 1 },
            direction: Direction::Both,
            ts: 2,
            carrier_num: 1584,
            call_id: 10,
            usage: 4,
            circuit_mode: CircuitModeType::TchS,
            comm_type: CommunicationType::P2p,
            simplex_duplex: true,
            speech_service: Some(0),
            etee_encrypted: false,
        });
        mgr.dl.push(CmceCircuit {
            ts_created: TdmaTime { h: 0, m: 1, f: 1, t: 1 },
            direction: Direction::Both,
            ts: 3,
            carrier_num: 1584,
            call_id: 11,
            usage: 5,
            circuit_mode: CircuitModeType::TchS,
            comm_type: CommunicationType::P2p,
            simplex_duplex: false,
            speech_service: Some(0),
            etee_encrypted: false,
        });

        let later = TdmaTime { h: 0, m: 8, f: 1, t: 1 };
        let tasks = mgr.tick_start(later);

        let closes: Vec<_> = tasks
            .unwrap_or_default()
            .into_iter()
            .filter_map(|cmd| match cmd {
                CircuitMgrCmd::SendClose(_, circuit) => Some(circuit),
                _ => None,
            })
            .collect();
        assert_eq!(closes.len(), 1);
        assert_eq!(closes[0].ts, 3);
        assert!(!closes[0].simplex_duplex);
        assert!(mgr.is_active_dir_slot(1584, 2, Direction::Dl));
        assert!(!mgr.is_active_dir_slot(1584, 3, Direction::Dl));
    }
}
