use tetra_config::bluestation::SharedConfig;
use tetra_config::bluestation::sec_cell::CfgBsServiceDetails;
use tetra_core::{BitBuffer, Sap, SsiType, TetraAddress, tetra_entities::TetraEntity};
use tetra_pdus::mle::{
    enums::mle_protocol_discriminator::MleProtocolDiscriminator, fields::bs_service_details::BsServiceDetails,
    fields::neighbour_cell_information_for_ca::NeighbourCellInformationForCa, pdus::d_nwrk_broadcast::DNwrkBroadcast,
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
                // Send broadcast if either time OR neighbor cells are configured.
                // This lets us advertise neighbors even without a timezone.
                if self.time_broadcast.is_some() || !self.config.config().cell.neighbor_cells_ca.is_empty() {
                    BroadcastType::NetworkTime
                } else {
                    BroadcastType::None
                }
            }
            BroadcastType::NetworkTime => BroadcastType::NetworkTime,
        }
    }

    fn send_d_nwrk_broadcast(&self, queue: &mut MessageQueue) {
        // Encode time if a timezone is configured. If encoding fails (e.g. DST
        // ambiguity or invalid timezone), continue without time — the broadcast
        // PDU still carries useful info (neighbours, cell load) and must not crash.
        let time_value = self.time_broadcast.as_deref().and_then(network_time::encode_tetra_network_time);
        let cfg = self.config.config();
        let has_neighbours = !cfg.cell.neighbor_cells_ca.is_empty();

        // Strategy:
        //   - If no neighbours: send a single time-only PDU (compact, ~75 bits).
        //   - If neighbours configured: send a single combined PDU (time + neighbours).
        //
        // We deliberately do NOT send two PDUs (time-only + with-neighbours) when NCB is
        // active. Each D-NWRK-BROADCAST occupies a slot on the MCCH BNCH, and the MCCH
        // is shared with other broadcasts (Home Mode Display SDS, periodic SDS, live SDS).
        // Sending 2 PDUs per hyperframe doubled MCCH pressure and squeezed HMD out of the
        // queue — radios stopped showing the configured callsign as soon as neighbours were
        // configured.
        if has_neighbours {
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
                    timeshare_cell_information_or_security_parameters: c.timeshare_cell_information_or_security_parameters,
                    tdma_frame_offset: c.tdma_frame_offset,
                })
                .collect();

            let neighbour_count = neighbour_cells.len() as u8;
            let pdu_with_neighbours = DNwrkBroadcast {
                cell_re_select_parameters: 0,
                cell_load_ca: 0,
                tetra_network_time: time_value,
                number_of_ca_neighbour_cells: Some(neighbour_count),
                neighbour_cell_information_for_ca: neighbour_cells,
            };
            self.transmit_pdu(queue, pdu_with_neighbours, &format!("time+{} neighbours", neighbour_count));
        } else if let Some(time_value) = time_value {
            // Time only, no neighbours — keep the field semantics identical to BlueStation
            // for maximum MS compatibility: number_of_ca_neighbour_cells = Some(0).
            let pdu_time_only = DNwrkBroadcast {
                cell_re_select_parameters: 0,
                cell_load_ca: 0,
                tetra_network_time: Some(time_value),
                number_of_ca_neighbour_cells: Some(0),
                neighbour_cell_information_for_ca: Vec::new(),
            };
            self.transmit_pdu(queue, pdu_time_only, "time-only");
        }

        tracing::info!(
            "D-NWRK-BROADCAST sent (tz={:?}, time={}, neighbours={})",
            self.time_broadcast,
            time_value
                .map(|value| format!("0x{value:012X}"))
                .unwrap_or_else(|| "disabled".to_string()),
            cfg.cell.neighbor_cells_ca.len()
        );
    }

    /// Common transmission path — serializes the PDU, prepends the MLE
    /// protocol discriminator, and pushes a TLA-UNITDATA req onto the queue.
    fn transmit_pdu(&self, queue: &mut MessageQueue, pdu: DNwrkBroadcast, label: &str) {
        // Use autoexpand buffer — with up to 7 fully-populated neighbour cells the PDU
        // can exceed 900 bits, so a fixed-size buffer would silently corrupt the output.
        let mut pdu_buf = BitBuffer::new_autoexpand(256);
        if let Err(e) = pdu.to_bitbuf(&mut pdu_buf) {
            tracing::warn!("Failed to serialize D-NWRK-BROADCAST ({}): {:?}", label, e);
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
        tracing::debug!("D-NWRK-BROADCAST [{}] queued ({} bits)", label, pdu_len);
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
