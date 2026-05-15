use tetra_config::bluestation::SharedConfig;
use tetra_config::bluestation::sec_cell::CfgBsServiceDetails;
use tetra_core::{BitBuffer, Sap, SsiType, TetraAddress, tetra_entities::TetraEntity};
use tetra_pdus::mle::{
    enums::mle_protocol_discriminator::MleProtocolDiscriminator,
    fields::bs_service_details::BsServiceDetails,
    fields::neighbour_cell_information_for_ca::NeighbourCellInformationForCa,
    pdus::d_nwrk_broadcast::DNwrkBroadcast,
};
use tetra_saps::{SapMsg, SapMsgInner, tla::TlaTlUnitdataReqBl};

use crate::{MessageQueue, mle::components::network_time};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BroadcastType {
    /// Initial value and value when no broadcast types are enabled
    None,
    NetworkTime,
}

pub struct MleBroadcast {
    config: SharedConfig,
    last_broadcast_type: BroadcastType,
    time_broadcast: Option<String>,
}

impl MleBroadcast {
    pub fn new(config: SharedConfig) -> Self {
        let time_broadcast = config.config().cell.timezone.clone();
        Self {
            config,
            last_broadcast_type: BroadcastType::None,
            time_broadcast,
        }
    }

    /// Send the next broadcast message based on the configured broadcast types and internal state.
    pub fn send_broadcast(&mut self, queue: &mut MessageQueue) {
        let broadcast_type = self.determine_next_broadcast_type();
        self.last_broadcast_type = broadcast_type;

        match broadcast_type {
            BroadcastType::NetworkTime => {
                self.send_d_nwrk_broadcast(queue);
            }
            BroadcastType::None => {
                // No broadcast to send
            }
        }
    }

    /// Determines the next type for the next broadcast message
    fn determine_next_broadcast_type(&self) -> BroadcastType {
        match self.last_broadcast_type {
            BroadcastType::None => {
                if self.time_broadcast.is_some() {
                    BroadcastType::NetworkTime
                } else {
                    BroadcastType::None
                }
            }
            BroadcastType::NetworkTime => BroadcastType::NetworkTime,
        }
    }

    fn send_d_nwrk_broadcast(&self, queue: &mut MessageQueue) {
        // Timezone is validated at config parse time, so encode cannot fail here
        let tz = self.time_broadcast.as_deref().unwrap();
        let time_value = match network_time::encode_tetra_network_time(tz) {
            Some(v) => v,
            None => {
                tracing::warn!("D-NWRK-BROADCAST: failed to encode network time for tz='{}', skipping", tz);
                return;
            }
        };

        // Build neighbor cell list from config
        let cfg = self.config.config();
        let neighbour_cells: Vec<NeighbourCellInformationForCa> = cfg
            .cell
            .neighbor_cells_ca
            .iter()
            .map(|c| NeighbourCellInformationForCa {
                cell_identifier_ca: c.cell_identifier_ca,
                cell_reselection_types_supported: c.cell_reselection_types_supported,
                neighbour_cell_synchronized: c.neighbor_cell_synchronized,
                cell_load_ca: c.cell_load_ca,
                main_carrier_number: c.main_carrier_number,
                main_carrier_number_extension: c.main_carrier_number_extension,
                mcc: c.mcc,
                mnc: c.mnc,
                location_area: c.location_area,
                maximum_ms_transmit_power: c.maximum_ms_transmit_power,
                minimum_rx_access_level: c.minimum_rx_access_level,
                subscriber_class: c.subscriber_class,
                bs_service_details: c.bs_service_details.as_ref().map(cfg_to_bs_service_details),
                timeshare_cell_information_or_security_parameters: c
                    .timeshare_cell_information_or_security_parameters,
                tdma_frame_offset: c.tdma_frame_offset,
            })
            .collect();

        let neighbour_count = neighbour_cells.len() as u8;

        // Use Some(0) when no neighbours — matches BlueStation behaviour and what
        // Motorola radios expect. None (field absent) causes radios to reject the PDU.
        let number_of_ca_neighbour_cells = if neighbour_count > 0 {
            Some(neighbour_count)
        } else {
            Some(0)
        };

        let pdu = DNwrkBroadcast {
            cell_re_select_parameters: 0,
            cell_load_ca: 0,
            tetra_network_time: Some(time_value),
            number_of_ca_neighbour_cells,
            neighbour_cell_information_for_ca: neighbour_cells,
        };

        // Use autoexpand buffer — with up to 7 fully-populated neighbour cells the PDU
        // can exceed 900 bits, so a fixed-size buffer would silently corrupt the output.
        let mut pdu_buf = BitBuffer::new_autoexpand(256);
        if let Err(e) = pdu.to_bitbuf(&mut pdu_buf) {
            tracing::warn!("Failed to serialize D-NWRK-BROADCAST: {:?}", e);
            return;
        }
        let pdu_len = pdu_buf.get_pos();
        pdu_buf.seek(0);

        // Prepend 3-bit MLE protocol discriminator
        let mut tl_sdu = BitBuffer::new(3 + pdu_len);
        tl_sdu.write_bits(MleProtocolDiscriminator::Mle.into_raw(), 3);
        tl_sdu.copy_bits(&mut pdu_buf, pdu_len);
        tl_sdu.seek(0);

        let sapmsg = SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Mle,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TlaTlUnitdataReqBl(TlaTlUnitdataReqBl {
                main_address: TetraAddress {
                    ssi: 0xFFFFFF,
                    ssi_type: SsiType::Gssi,
                },
                link_id: 0,
                endpoint_id: 0,
                tl_sdu,
                stealing_permission: false,
                subscriber_class: 0,
                fcs_flag: false,
                air_interface_encryption: None,
                packet_data_flag: false,
                n_tlsdu_repeats: 0,
                data_class_info: None,
                req_handle: 0,
                chan_alloc: None,
                tx_reporter: None,
            }),
        };
        queue.push_back(sapmsg);
        tracing::info!(
            "D-NWRK-BROADCAST sent (tz={}, time=0x{:012X}, neighbours={})",
            tz, time_value, neighbour_count
        );
    }
}

fn cfg_to_bs_service_details(c: &CfgBsServiceDetails) -> BsServiceDetails {
    BsServiceDetails {
        registration: c.registration,
        deregistration: c.deregistration,
        priority_cell: c.priority_cell,
        no_minimum_mode: c.no_minimum_mode,
        migration: c.migration,
        system_wide_services: c.system_wide_services,
        voice_service: c.voice_service,
        circuit_mode_data_service: c.circuit_mode_data_service,
        sndcp_service: c.sndcp_service,
        aie_service: c.aie_service,
        advanced_link: c.advanced_link,
    }
}
