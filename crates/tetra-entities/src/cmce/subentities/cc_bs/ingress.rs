use super::*;

impl CcBsSubentity {
    pub fn route_xx_deliver(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("route_xx_deliver");

        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            panic!();
        };
        let Some(bits) = prim.sdu.peek_bits(5) else {
            tracing::warn!("insufficient bits: {}", prim.sdu.dump_bin());
            return;
        };
        let Ok(pdu_type) = CmcePduTypeUl::try_from(bits) else {
            tracing::warn!("invalid pdu type: {} in {}", bits, prim.sdu.dump_bin());
            return;
        };

        match pdu_type {
            CmcePduTypeUl::USetup => self.rx_u_setup(queue, message),
            CmcePduTypeUl::UTxCeased => self.rx_u_tx_ceased(queue, message),
            CmcePduTypeUl::UTxDemand => self.rx_u_tx_demand(queue, message),
            CmcePduTypeUl::URelease => self.rx_u_release(queue, message),
            CmcePduTypeUl::UDisconnect => self.rx_u_disconnect(queue, message),
            CmcePduTypeUl::UAlert => self.rx_u_alert(queue, message),
            CmcePduTypeUl::UConnect => self.rx_u_connect(queue, message),
            CmcePduTypeUl::UInfo => self.rx_u_info(queue, message),
            CmcePduTypeUl::UStatus | CmcePduTypeUl::UCallRestore => {
                unimplemented_log!("{}", pdu_type);
            }
            _ => {
                panic!();
            }
        }
    }

    pub fn rx_call_control(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        let SapMsgInner::CmceCallControl(call_control) = message.msg else {
            panic!("Expected CmceCallControl message");
        };

        match call_control {
            CallControl::NetworkCallStart {
                brew_uuid,
                source_issi,
                dest_gssi,
                priority,
            } => {
                self.rx_network_call_start(queue, brew_uuid, source_issi, dest_gssi, priority);
            }
            CallControl::NetworkCallEnd { brew_uuid } => {
                self.rx_network_call_end(queue, brew_uuid);
            }
            CallControl::NetworkCircuitSetupRequest { brew_uuid, call } => {
                self.rx_network_circuit_setup_request(queue, brew_uuid, call);
            }
            CallControl::NetworkCircuitSetupAccept { brew_uuid } => {
                self.rx_network_circuit_setup_accept(brew_uuid);
            }
            CallControl::NetworkCircuitSetupReject { brew_uuid, cause } => {
                self.rx_network_circuit_setup_reject(queue, brew_uuid, cause);
            }
            CallControl::NetworkCircuitAlert { brew_uuid } => {
                self.rx_network_circuit_alert(queue, brew_uuid);
            }
            CallControl::NetworkCircuitConnectRequest { brew_uuid, call } => {
                self.rx_network_circuit_connect_request(queue, brew_uuid, call);
            }
            CallControl::NetworkCircuitConnectConfirm {
                brew_uuid,
                grant,
                permission,
            } => {
                self.rx_network_circuit_connect_confirm(queue, brew_uuid, grant, permission);
            }
            CallControl::NetworkCircuitMediaReady { brew_uuid, .. } => {
                tracing::trace!("CMCE: ignoring unexpected NetworkCircuitMediaReady uuid={}", brew_uuid);
            }
            CallControl::NetworkCircuitRelease { brew_uuid, cause } => {
                self.rx_network_circuit_release(queue, brew_uuid, cause);
            }
            CallControl::UlInactivityTimeout { ts } => {
                self.handle_ul_inactivity_timeout(queue, ts);
            }
            _ => {
                tracing::warn!("Unexpected CallControl message: {:?}", call_control);
            }
        }
    }

    pub(super) fn rx_u_setup(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_u_setup: {:?}", message);
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            panic!()
        };
        let calling_party = prim.received_tetra_address;

        let pdu = match USetup::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-SETUP {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-SETUP: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        self.fsm_on_u_setup(queue, &message, &pdu, calling_party);
    }

    pub(super) fn rx_u_tx_ceased(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            panic!()
        };

        let sender = prim.received_tetra_address;
        let pdu = match UTxCeased::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-TX CEASED {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-TX CEASED: {:?}", e);
                return;
            }
        };

        self.fsm_on_u_tx_ceased(queue, sender, pdu);
    }

    pub(super) fn rx_u_tx_demand(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            panic!()
        };

        let requesting_party = prim.received_tetra_address;
        let pdu = match UTxDemand::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-TX DEMAND {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-TX DEMAND: {:?}", e);
                return;
            }
        };

        self.fsm_on_u_tx_demand(queue, requesting_party, pdu);
    }

    pub(super) fn rx_u_release(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            panic!()
        };

        let sender = prim.received_tetra_address;
        let pdu = match URelease::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-RELEASE {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-RELEASE: {:?}", e);
                return;
            }
        };

        self.fsm_on_u_release(queue, sender, pdu);
    }

    pub(super) fn rx_u_disconnect(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            panic!()
        };

        let sender = prim.received_tetra_address;
        let ul_handle = prim.handle;
        let ul_link_id = prim.link_id;
        let ul_endpoint_id = prim.endpoint_id;

        let pdu = match UDisconnect::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-DISCONNECT {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-DISCONNECT: {:?}", e);
                return;
            }
        };

        self.fsm_on_u_disconnect(queue, sender, ul_handle, ul_link_id, ul_endpoint_id, pdu);
    }

    pub(super) fn rx_u_alert(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            panic!()
        };

        let pdu = match UAlert::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-ALERT {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-ALERT: {:?}", e);
                return;
            }
        };

        self.fsm_on_u_alert(queue, prim.received_tetra_address, prim.handle, prim.link_id, prim.endpoint_id, pdu);
    }

    /// Handle U-CONNECT for an individual call.
    pub(super) fn rx_u_connect(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            panic!()
        };

        let pdu = match UConnect::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-CONNECT {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-CONNECT: {:?}", e);
                return;
            }
        };

        self.fsm_on_u_connect(
            queue,
            prim.received_tetra_address,
            prim.handle,
            prim.link_id,
            prim.endpoint_id,
            pdu,
        );
    }

    pub(super) fn rx_u_info(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            panic!()
        };

        let pdu = match UInfo::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-INFO {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-INFO: {:?}", e);
                return;
            }
        };

        self.fsm_on_u_info(queue, pdu);
    }
}
