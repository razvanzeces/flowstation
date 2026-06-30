use super::*;

impl CcBsSubentity {
    pub fn rx_call_control(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        // The originating network entity (Brew or Asterisk). Inbound network-initiated setup
        // requests carry no prior call record, so we route replies back to the sender.
        let src_entity = message.src;
        let SapMsgInner::CmceCallControl(call_control) = message.msg else {
            tracing::warn!("CMCE CC control ingress received non-call-control message");
            return;
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
            CallControl::NetworkCallMediaActivity { brew_uuid } => {
                self.rx_network_call_media_activity(brew_uuid);
            }
            CallControl::UlInactivityTimeout { carrier_num, ts } => {
                self.handle_ul_inactivity_timeout_slot(queue, carrier_num, ts);
            }
            CallControl::NetworkCircuitSetupRequest { brew_uuid, call } => {
                self.rx_network_circuit_setup_request(queue, src_entity, brew_uuid, call);
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
            CallControl::NetworkCircuitSimplexGranted {
                brew_uuid,
                grant,
                permission,
            } => {
                self.rx_network_circuit_simplex_granted(queue, brew_uuid, grant, permission);
            }
            CallControl::NetworkCircuitSimplexIdle {
                brew_uuid,
                grant,
                permission,
            } => {
                self.rx_network_circuit_simplex_idle(queue, brew_uuid, grant, permission);
            }
            CallControl::NetworkCircuitMediaReady { brew_uuid, .. } => {
                tracing::trace!("CMCE: ignoring unexpected NetworkCircuitMediaReady uuid={}", brew_uuid);
            }
            CallControl::NetworkCircuitRelease { brew_uuid, cause } => {
                self.rx_network_circuit_release(queue, brew_uuid, cause);
            }
            _ => {
                tracing::warn!("Unexpected CallControl message: {:?}", call_control);
            }
        }
    }
}
