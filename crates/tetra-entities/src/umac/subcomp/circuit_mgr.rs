use std::collections::{HashMap, VecDeque};

use tetra_core::Direction;
use tetra_saps::control::call_control::{Circuit, CircuitDlMediaSource};

pub struct CircuitMgr {
    dl: HashMap<(u16, u8), Circuit>,
    ul: HashMap<(u16, u8), Circuit>,

    /// Data blocks queued to be transmitted, per carrier/timeslot.
    tx_data: HashMap<(u16, u8), VecDeque<Vec<u8>>>,
}

impl CircuitMgr {
    pub fn new() -> Self {
        Self {
            dl: HashMap::new(),
            ul: HashMap::new(),
            tx_data: HashMap::new(),
        }
    }

    fn key(carrier_num: u16, ts: u8) -> (u16, u8) {
        (carrier_num, ts)
    }

    pub fn is_active(&self, dir: Direction, carrier_num: u16, ts: u8) -> bool {
        let key = Self::key(carrier_num, ts);
        match dir {
            Direction::Dl => self.dl.contains_key(&key),
            Direction::Ul => self.ul.contains_key(&key),
            _ => {
                tracing::error!("UMAC CircuitMgr: called with non-specific direction {:?}", dir);
                false
            }
        }
    }

    pub fn get_usage(&self, dir: Direction, carrier_num: u16, ts: u8) -> Option<u8> {
        let key = Self::key(carrier_num, ts);
        match dir {
            Direction::Dl => self.dl.get(&key).map(|circuit| circuit.usage),
            Direction::Ul => self.ul.get(&key).map(|circuit| circuit.usage),
            _ => {
                tracing::error!("UMAC CircuitMgr: called with non-specific direction {:?}", dir);
                None
            }
        }
    }

    pub fn get_ul_peer_route(&self, carrier_num: u16, ts: u8) -> Option<(u16, u8)> {
        let key = Self::key(carrier_num, ts);
        let circuit = if let Some(dl) = self.dl.get(&key) {
            if dl.direction == Direction::Both {
                Some(dl)
            } else {
                self.ul.get(&key)
            }
        } else {
            self.ul.get(&key)
        };

        circuit.and_then(|c| c.peer_ts.map(|peer_ts| (c.peer_carrier_num.unwrap_or(carrier_num), peer_ts)))
    }

    pub fn get_dl_media_source(&self, carrier_num: u16, ts: u8) -> Option<CircuitDlMediaSource> {
        self.dl.get(&Self::key(carrier_num, ts)).map(|c| c.dl_media_source)
    }

    /// Closes an active circuit, and return the Circuit to the caller.
    pub fn close_circuit(&mut self, dir: Direction, carrier_num: u16, ts: u8) -> Option<Circuit> {
        let key = Self::key(carrier_num, ts);
        match dir {
            Direction::Dl => {
                self.tx_data.remove(&key);
                self.dl.remove(&key)
            }
            Direction::Ul => self.ul.remove(&key),
            _ => {
                tracing::error!("UMAC CircuitMgr: called with non-specific direction {:?}", dir);
                None
            }
        }
    }

    /// Creates a new circuit on the given direction and carrier/timeslot.
    /// This channel should be free, if not, warnings will be issued and the existing circuit will be closed first.
    pub fn create_circuit(&mut self, dir: Direction, circuit: Circuit) {
        let key = Self::key(circuit.carrier_num, circuit.ts);

        if self.is_active(dir, circuit.carrier_num, circuit.ts) {
            tracing::warn!(
                "CircuitMgr::create had still active circuit on {:?} carrier={} ts={}",
                dir,
                circuit.carrier_num,
                circuit.ts
            );
            self.close_circuit(dir, circuit.carrier_num, circuit.ts);
        }

        match dir {
            Direction::Dl => {
                if self.tx_data.get(&key).is_some_and(|queue| !queue.is_empty()) {
                    tracing::warn!(
                        "CircuitMgr::create had pending tx_data on Dl carrier={} ts={}",
                        circuit.carrier_num,
                        circuit.ts
                    );
                    self.tx_data.remove(&key);
                }
                self.dl.insert(key, circuit);
            }
            Direction::Ul => {
                self.ul.insert(key, circuit);
            }
            _ => {
                tracing::error!("UMAC CircuitMgr: called with non-specific direction {:?}", dir);
            }
        }
    }

    /// Put a block in the queue for transmission on an associated channel.
    pub fn put_block(&mut self, carrier_num: u16, ts: u8, block: Vec<u8>) {
        if !self.is_active(Direction::Dl, carrier_num, ts) {
            tracing::warn!(
                "CircuitMgr::put_block on inactive circuit {:?} carrier={} ts={}",
                Direction::Dl,
                carrier_num,
                ts
            );
            return;
        }
        self.tx_data.entry(Self::key(carrier_num, ts)).or_default().push_back(block);
    }

    /// Take a to-be-transmitted block from the queue.
    pub fn take_block(&mut self, carrier_num: u16, ts: u8) -> Option<Vec<u8>> {
        if !self.is_active(Direction::Dl, carrier_num, ts) {
            tracing::warn!(
                "CircuitMgr::take_block on inactive circuit {:?} carrier={} ts={}",
                Direction::Dl,
                carrier_num,
                ts
            );
            return None;
        }
        self.tx_data.get_mut(&Self::key(carrier_num, ts)).and_then(VecDeque::pop_front)
    }
}
