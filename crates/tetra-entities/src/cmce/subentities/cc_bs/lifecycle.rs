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
    ToEntityForLocalSource { entity: TetraEntity, source_issi: u32 },
    ForLocalSource { source_issi: u32, dest_gssi: u32 },
}

impl BrewNotification {
    fn destination(self, config: &SharedConfig) -> Option<TetraEntity> {
        match self {
            BrewNotification::Never => None,
            BrewNotification::ToEntityForLocalSource { entity, source_issi } => {
                if brew::is_active_for_entity(config, entity) && brew::is_brew_local_issi_allowed_for_entity(config, entity, source_issi) {
                    Some(entity)
                } else {
                    None
                }
            }
            BrewNotification::ForLocalSource { source_issi, dest_gssi } => {
                let entity = brew::route_entity_for_local_issi(config, source_issi)?;
                if brew::is_brew_gssi_routable_for_entity(config, entity, dest_gssi) {
                    Some(entity)
                } else {
                    None
                }
            }
        }
    }
}

impl CcBsSubentity {
    pub(super) fn brew_notification_for_group_call(call: &ActiveCall, local_source_issi: u32) -> BrewNotification {
        match &call.origin {
            CallOrigin::Network { network_entity, .. } => BrewNotification::ToEntityForLocalSource {
                entity: *network_entity,
                source_issi: local_source_issi,
            },
            CallOrigin::Local { .. } => BrewNotification::ForLocalSource {
                source_issi: local_source_issi,
                dest_gssi: call.dest_gssi,
            },
        }
    }

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

        if let Some(brew_entity) = notify_brew.destination(&self.config) {
            Self::push_control(
                queue,
                brew_entity,
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

        if let Some(brew_entity) = notify_brew.destination(&self.config) {
            Self::push_control(
                queue,
                brew_entity,
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

        if let Some(brew_entity) = notify_brew.destination(&self.config) {
            Self::push_control(
                queue,
                brew_entity,
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
