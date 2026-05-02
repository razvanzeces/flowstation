use super::*;

impl CcBsSubentity {
    pub(super) fn rx_network_circuit_setup_request(&mut self, queue: &mut MessageQueue, brew_uuid: uuid::Uuid, call: NetworkCircuitCall) {
        self.fsm_on_network_circuit_setup_request(queue, brew_uuid, call);
    }

    pub(super) fn rx_network_circuit_setup_accept(&mut self, brew_uuid: uuid::Uuid) {
        if self.find_brew_individual_call(brew_uuid).is_some() {
            tracing::info!("CMCE: Brew setup accepted uuid={}", brew_uuid);
        } else {
            tracing::debug!("CMCE: Brew setup accept for unknown uuid={}", brew_uuid);
        }
    }

    pub(super) fn rx_network_circuit_setup_reject(&mut self, queue: &mut MessageQueue, brew_uuid: uuid::Uuid, cause: u8) {
        let Some((call_id, _)) = self.find_brew_individual_call(brew_uuid) else {
            tracing::debug!("CMCE: Brew setup reject for unknown uuid={} cause={}", brew_uuid, cause);
            return;
        };
        let mapped = DisconnectCause::try_from(cause as u64).unwrap_or(DisconnectCause::RequestedServiceNotAvailable);
        tracing::info!(
            "CMCE: Brew setup rejected uuid={} call_id={} cause={} ({:?})",
            brew_uuid,
            call_id,
            cause,
            mapped
        );
        self.release_individual_call(queue, call_id, mapped);
    }

    pub(super) fn rx_network_circuit_alert(&mut self, queue: &mut MessageQueue, brew_uuid: uuid::Uuid) {
        let Some((call_id, _call)) = self.find_brew_individual_call(brew_uuid) else {
            tracing::debug!("CMCE: Brew alert for unknown uuid={}", brew_uuid);
            return;
        };

        if let Err(err) = self.fsm_individual_on_alert(queue, call_id, None, CallTimeoutSetupPhase::T60s) {
            match err {
                IndividualTransitionError::UnknownCall(_) => {
                    tracing::debug!("CMCE: Brew alert for unknown call_id={} uuid={}", call_id, brew_uuid);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::trace!(
                        "CMCE: Brew alert ignored call_id={} uuid={} invalid from state {:?}",
                        call_id,
                        brew_uuid,
                        state
                    );
                }
                IndividualTransitionError::MissingBrewUuid(_) => {
                    tracing::warn!("CMCE: Brew alert missing brew_uuid on call_id={}", call_id);
                }
                IndividualTransitionError::DuplicateCall(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }
    }

    pub(super) fn rx_network_circuit_connect_request(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        call_info: NetworkCircuitCall,
    ) {
        self.fsm_on_network_circuit_connect_request(queue, brew_uuid, call_info);
    }

    pub(super) fn rx_network_circuit_connect_confirm(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        grant: u8,
        permission: u8,
    ) {
        self.fsm_on_network_circuit_connect_confirm(queue, brew_uuid, grant, permission);
    }

    pub(super) fn rx_network_circuit_release(&mut self, queue: &mut MessageQueue, brew_uuid: uuid::Uuid, cause: u8) {
        let Some((call_id, _)) = self.find_brew_individual_call(brew_uuid) else {
            tracing::debug!("CMCE: Brew release for unknown uuid={} cause={}", brew_uuid, cause);
            return;
        };
        let mapped = DisconnectCause::try_from(cause as u64).unwrap_or(DisconnectCause::SwmiRequestedDisconnection);
        tracing::info!(
            "CMCE: Brew release uuid={} call_id={} cause={} ({:?})",
            brew_uuid,
            call_id,
            cause,
            mapped
        );
        self.release_individual_call(queue, call_id, mapped);
    }

    /// Handle network-initiated group call start
    pub(super) fn rx_network_call_start(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        source_issi: u32,
        dest_gssi: u32,
        priority: u8,
    ) {
        self.fsm_on_network_call_start(queue, brew_uuid, source_issi, dest_gssi, priority);
    }

    /// Handle network call end request
    pub(super) fn rx_network_call_end(&mut self, queue: &mut MessageQueue, brew_uuid: uuid::Uuid) {
        // Find the call by brew_uuid field (works for both Local and Network origin calls)
        let Some((call_id, call)) = self
            .active_calls
            .iter()
            .find(|(_, c)| c.brew_uuid == Some(brew_uuid))
            .map(|(id, c)| (*id, c.clone()))
        else {
            tracing::debug!("CMCE: network call end for unknown brew_uuid={}", brew_uuid);
            return;
        };

        tracing::info!(
            "CMCE: network call ended brew_uuid={} call_id={} gssi={}",
            brew_uuid,
            call_id,
            call.dest_gssi
        );

        if let Err(err) = self.fsm_group_on_network_call_end(queue, call_id) {
            match err {
                GroupTransitionError::UnknownCall(_) => {
                    tracing::debug!("CMCE: network call end for unknown call_id={} brew_uuid={}", call_id, brew_uuid);
                }
                GroupTransitionError::InvalidTransition { state, .. } => {
                    tracing::warn!("CMCE: network call end rejected call_id={} from state {:?}", call_id, state);
                }
                GroupTransitionError::NotCurrentSpeaker { .. } => {
                    tracing::debug!(
                        "CMCE: network call end produced unexpected NotCurrentSpeaker for call_id={}",
                        call_id
                    );
                }
                GroupTransitionError::MissingCachedSetup(_) => {
                    tracing::debug!("CMCE: network call end call_id={} missing cached setup", call_id);
                }
            }
        }
    }
}
