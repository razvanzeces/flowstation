use std::collections::{HashMap, HashSet};

use crate::net_telemetry::{TelemetryEvent, channel::TelemetrySink};
use tetra_pdus::mm::enums::energy_saving_mode::EnergySavingMode;
use tetra_pdus::mm::fields::class_of_ms::ClassOfMs;

#[derive(Debug)]
pub enum ClientMgrErr {
    ClientNotFound { issi: u32 },
    GroupNotFound { gssi: u32 },
    IssiInGroupRange { issi: u32 },
    GssiInClientRange { gssi: u32 },
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum MmClientState {
    Unknown,
    Attached,
    Detached,
}

pub struct MmClientProperties {
    pub issi: u32,
    pub state: MmClientState,
    pub groups: HashSet<u32>,
    pub energy_saving_mode: EnergySavingMode,
    pub class_of_ms: Option<ClassOfMs>,
    /// Layer-2 handle from the last successful location update.
    /// Required for sending downlink MM PDUs (D-LOCATION-UPDATE-COMMAND etc.)
    /// to this MS. Set to 0 until the first location update is received.
    pub last_handle: u32,
    /// Terminal Equipment Identity (60-bit hardware ID, like IMEI).
    /// Set when the MS sends U-TEI-PROVIDE. None if not yet received.
    pub tei: Option<u64>,
    // pub last_seen: TdmaTime,
}

impl MmClientProperties {
    pub fn new(ssi: u32) -> Self {
        MmClientProperties {
            issi: ssi,
            state: MmClientState::Unknown,
            groups: HashSet::new(),
            energy_saving_mode: EnergySavingMode::StayAlive,
            class_of_ms: None,
            last_handle: 0,
            tei: None,
            // last_seen: TdmaTime::default(),
        }
    }
}

/// Stub function, to be replaced with checks based on configuration file
fn is_individual(_issi: u32) -> bool {
    return true;
}
/// Stub function, to be replaced with checks based on configuration file
fn in_group_range(_gssi: u32) -> bool {
    return true;
}
/// Stub function, to be replaced with checks based on configuration file
fn is_group(_gssi: u32) -> bool {
    return true;
}
/// Stub function, to be replaced with checks based on configuration file
fn may_attach(_issi: u32, _gssi: u32) -> bool {
    return true;
}

pub struct MmClientMgr {
    clients: HashMap<u32, MmClientProperties>,
    telemetry_sink: Option<TelemetrySink>,
}

impl MmClientMgr {
    pub fn new(telemetry_sink: Option<TelemetrySink>) -> Self {
        MmClientMgr {
            clients: HashMap::new(),
            telemetry_sink,
        }
    }

    pub fn get_client_by_issi(&mut self, issi: u32) -> Option<&MmClientProperties> {
        self.clients.get(&issi)
    }

    pub fn client_is_known(&self, issi: u32) -> bool {
        self.clients.contains_key(&issi)
    }

    pub fn set_client_state(&mut self, issi: u32, state: MmClientState) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.state = state;
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    pub fn set_client_energy_saving_mode(&mut self, issi: u32, mode: EnergySavingMode) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.energy_saving_mode = mode;
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    pub fn set_client_class_of_ms(&mut self, issi: u32, class: Option<ClassOfMs>) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.class_of_ms = class;
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    /// Store the TEI (Terminal Equipment Identity) received from U-TEI-PROVIDE.
    /// If the ISSI is not registered yet, the TEI is silently ignored (can't fail critically).
    pub fn set_client_tei(&mut self, issi: u32, tei: u64) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.tei = Some(tei);
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    /// Registers a fresh state for a client, based on ssi
    /// If client is already registered, previous state is discarded.
    pub fn try_register_client(&mut self, issi: u32, attached: bool) -> Result<bool, ClientMgrErr> {
        if !is_individual(issi) {
            return Err(ClientMgrErr::IssiInGroupRange { issi });
        };

        // discard previous state if any
        self.clients.remove(&issi);

        // Create and insert new client state
        let mut elem = MmClientProperties::new(issi);
        elem.state = if attached {
            MmClientState::Attached
        } else {
            MmClientState::Unknown
        };
        self.clients.insert(issi, elem);

        // Send telemetry event
        if let Some(sink) = &self.telemetry_sink {
            sink.send(TelemetryEvent::MsRegistration { issi });
        }

        Ok(true)
    }

    /// Removes a client from the registry, returning its properties if found
    /// Returns (issi, last_handle) for all known clients that have a valid handle (handle != 0).
    /// Used by mm_bs to send D-LOCATION-UPDATE-COMMAND after Brew reconnection.
    pub fn all_clients_with_handle(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.clients
            .values()
            .filter(|c| c.last_handle != 0)
            .map(|c| (c.issi, c.last_handle))
    }

    /// Update the last known L2 handle for a registered client.
    pub fn set_client_handle(&mut self, issi: u32, handle: u32) {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.last_handle = handle;
        }
    }

    pub fn remove_client(&mut self, ssi: u32) -> Option<MmClientProperties> {
        if let Some(client) = self.clients.remove(&ssi) {
            // Send telemetry event
            if let Some(sink) = &self.telemetry_sink {
                sink.send(TelemetryEvent::MsDeregistration { issi: ssi });
            }
            Some(client)
        } else {
            None
        }
    }

    /// Detaches all groups from a client
    pub fn client_detach_all_groups(&mut self, issi: u32) -> Result<bool, ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            // Send telemetry event
            if let Some(sink) = &self.telemetry_sink {
                sink.send(TelemetryEvent::MsGroupDetach {
                    issi: client.issi,
                    gssis: client.groups.iter().cloned().collect(),
                });
            }
            client.groups.clear();
            Ok(true)
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    /// Attaches or detaches a client from a group
    pub fn client_group_attach(&mut self, issi: u32, gssi: u32, do_attach: bool) -> Result<bool, ClientMgrErr> {
        // Checks
        if !in_group_range(gssi) {
            return Err(ClientMgrErr::GssiInClientRange { gssi });
        };
        if !is_group(gssi) {
            return Err(ClientMgrErr::GroupNotFound { gssi });
        };
        if !may_attach(issi, gssi) {
            return Err(ClientMgrErr::GroupNotFound { gssi });
        };

        if let Some(client) = self.clients.get_mut(&issi) {
            if do_attach {
                // Send telemetry event
                if let Some(sink) = &self.telemetry_sink {
                    sink.send(TelemetryEvent::MsGroupAttach {
                        issi: client.issi,
                        gssis: vec![gssi].into_iter().collect(),
                    });
                }

                Ok(client.groups.insert(gssi))
            } else {
                Ok(client.groups.remove(&gssi))
            }
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }
}
