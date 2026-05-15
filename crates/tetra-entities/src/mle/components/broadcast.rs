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

        // Log starea initiala la startup — primul semn vizibil ca feature-ul e activ sau nu
        match &time_broadcast {
            Some(tz) => tracing::info!(
                "MLE broadcast: D-NWRK-BROADCAST activ, timezone='{}' (broadcast o data per hyperframe ~61s)",
                tz
            ),
            None => tracing::info!(
                "MLE broadcast: D-NWRK-BROADCAST inactiv — cell.timezone neconfigurat in config.toml"
            ),
        }

        Self {
            config,
            last_broadcast_type: BroadcastType::None,
            time_broadcast,
        }
    }

    /// Send the next broadcast message based on the configured broadcast types and internal state.
    pub fn send_broadcast(&mut self, queue: &mut MessageQueue) {
        let broadcast_type = self.determine_next_broadcast_type();

        tracing::debug!(
            "MLE broadcast: slot fired, last={:?} -> next={:?}",
            self.last_broadcast_type,
            broadcast_type
        );

        self.last_broadcast_type = broadcast_type;

        match broadcast_type {
            BroadcastType::NetworkTime => {
                self.send_d_nwrk_broadcast(queue);
            }
            BroadcastType::None => {
                tracing::debug!("MLE broadcast: nimic de trimis (timezone neconfigurat)");
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
        let tz = self.time_broadcast.as_deref().unwrap();

        tracing::debug!("D-NWRK-BROADCAST: encoding time pentru tz='{}'", tz);

        // FIX: timezone-ul a trecut validarea din SharedConfig::from_parts() la startup,
        // deci encode_tetra_network_time() nu ar trebui sa returneze niciodata None aici.
        // Daca se intampla totusi, este un bug grav (chrono feature lipsit, divergenta
        // de versiune chrono-tz) si trebuie sa fie vizibil imediat, nu ignorat silentios.
        //
        // Versiunea anterioara facea `return` cu un simplu `warn`, ceea ce insemna ca
        // PDU-ul nu se trimitea niciodata fara nicio notificare vizibila pentru utilizator.
        let time_value = network_time::encode_tetra_network_time(tz)
            .unwrap_or_else(|| {
                panic!(
                    "D-NWRK-BROADCAST: encode_tetra_network_time returned None pentru tz='{}' \
                     desi timezone-ul a trecut validarea la startup. \
                     Verifica ca chrono este compilat cu feature 'std' (vezi workspace Cargo.toml).",
                    tz
                )
            });

        tracing::debug!(
            "D-NWRK-BROADCAST: time encodat 0x{:012X} (sign={} offset_x15min={} year={})",
            time_value,
            (time_value >> 23) & 1,
            (time_value >> 17) & 0x3F,
            (time_value >> 11) & 0x3F,
        );

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
            tracing::warn!("D-NWRK-BROADCAST: serializare esuata: {:?}", e);
            return;
        }
        let pdu_len = pdu_buf.get_pos();
        pdu_buf.seek(0);

        tracing::debug!("D-NWRK-BROADCAST: PDU serializat ({} bits)", pdu_len);

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

        // INFO-level: confirmare clara ca PDU-ul a fost pus in coada spre LLC -> UMAC -> PHY.
        // Offset-ul e afisat si in ore pentru a putea verifica vizual DST-ul (UTC+2 iarna, UTC+3 vara).
        tracing::info!(
            "D-NWRK-BROADCAST queued OK \
             (tz={}, time=0x{:012X}, offset={}x15min={:.2}h, neighbours={})",
            tz,
            time_value,
            (time_value >> 17) & 0x3F,
            ((time_value >> 17) & 0x3F) as f32 * 0.25,
            neighbour_count
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
