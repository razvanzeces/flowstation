use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GroupFloorGrant {
    pub(super) call_id: u16,
    pub(super) source_issi: u32,
    pub(super) dest_gssi: u32,
    pub(super) carrier_num: u16,
    pub(super) ts: u8,
    pub(super) is_group: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CallTimeslot {
    pub(super) call_id: u16,
    pub(super) carrier_num: u16,
    pub(super) ts: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BrewNotification {
    Never,
    IfGroupRoutable(u32),
}

impl BrewNotification {
    fn enabled(self, config: &SharedConfig) -> bool {
        match self {
            BrewNotification::Never => false,
            BrewNotification::IfGroupRoutable(gssi) => brew::is_brew_gssi_routable(config, gssi),
        }
    }
}

impl CcBsSubentity {
    fn push_control(queue: &mut MessageQueue, dest: TetraEntity, control: CallControl) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest,
            msg: SapMsgInner::CmceCallControl(control),
        });
    }

    pub(super) fn notify_floor_granted(
        &self,
        queue: &mut MessageQueue,
        grant: GroupFloorGrant,
        notify_umac: bool,
        notify_brew: BrewNotification,
    ) {
        self.emit(crate::net_telemetry::TelemetryEvent::CallSpeakerChanged {
            call_id: grant.call_id,
            is_group: grant.is_group,
            dest_addr: grant.dest_gssi,
            speaker_issi: grant.source_issi,
            carrier_num: grant.carrier_num,
            ts: grant.ts,
        });

        if notify_umac {
            Self::push_control(
                queue,
                TetraEntity::Umac,
                CallControl::FloorGranted {
                    call_id: grant.call_id,
                    source_issi: grant.source_issi,
                    dest_gssi: grant.dest_gssi,
                    carrier_num: grant.carrier_num,
                    ts: grant.ts,
                },
            );
        }

        if notify_brew.enabled(&self.config) {
            Self::push_control(
                queue,
                TetraEntity::Brew,
                CallControl::FloorGranted {
                    call_id: grant.call_id,
                    source_issi: grant.source_issi,
                    dest_gssi: grant.dest_gssi,
                    carrier_num: grant.carrier_num,
                    ts: grant.ts,
                },
            );
        }
    }

    pub(super) fn notify_remote_floor_granted(&self, queue: &mut MessageQueue, slot: CallTimeslot) {
        Self::push_control(
            queue,
            TetraEntity::Umac,
            CallControl::RemoteFloorGranted {
                call_id: slot.call_id,
                carrier_num: slot.carrier_num,
                ts: slot.ts,
            },
        );
    }

    pub(super) fn notify_floor_released(
        &self,
        queue: &mut MessageQueue,
        slot: CallTimeslot,
        notify_umac: bool,
        notify_brew: BrewNotification,
    ) {
        if notify_umac {
            Self::push_control(
                queue,
                TetraEntity::Umac,
                CallControl::FloorReleased {
                    call_id: slot.call_id,
                    carrier_num: slot.carrier_num,
                    ts: slot.ts,
                },
            );
        }

        if notify_brew.enabled(&self.config) {
            Self::push_control(
                queue,
                TetraEntity::Brew,
                CallControl::FloorReleased {
                    call_id: slot.call_id,
                    carrier_num: slot.carrier_num,
                    ts: slot.ts,
                },
            );
        }
    }

    pub(super) fn notify_call_ended(&self, queue: &mut MessageQueue, slot: CallTimeslot, notify_umac: bool, notify_brew: BrewNotification) {
        if notify_umac {
            Self::push_control(
                queue,
                TetraEntity::Umac,
                CallControl::CallEnded {
                    call_id: slot.call_id,
                    carrier_num: slot.carrier_num,
                    ts: slot.ts,
                },
            );
        }

        if notify_brew.enabled(&self.config) {
            Self::push_control(
                queue,
                TetraEntity::Brew,
                CallControl::CallEnded {
                    call_id: slot.call_id,
                    carrier_num: slot.carrier_num,
                    ts: slot.ts,
                },
            );
        }
    }

    pub(super) fn notify_network_call_end(&self, queue: &mut MessageQueue, network_entity: TetraEntity, brew_uuid: uuid::Uuid) {
        Self::push_control(queue, network_entity, CallControl::NetworkCallEnd { brew_uuid });
    }

    pub(super) fn notify_network_circuit_release(
        &self,
        queue: &mut MessageQueue,
        network_entity: TetraEntity,
        brew_uuid: uuid::Uuid,
        cause: DisconnectCause,
    ) {
        Self::push_control(
            queue,
            network_entity,
            CallControl::NetworkCircuitRelease {
                brew_uuid,
                cause: cause.into_raw() as u8,
            },
        );
    }
}
