use core::fmt;

use tetra_core::typed_pdu_fields::typed;
use tetra_core::typed_pdu_fields::delimiters;
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mle::fields::bs_service_details::BsServiceDetails;

/// Clause 18.5.17: D-NWRK-BROADCAST Neighbor cell information for CA element (Table 18.64).
#[derive(Debug, Clone)]
pub struct NeighbourCellInformationForCa {
    /// Type1, 5 bits
    pub cell_identifier_ca: u8,
    /// Type1, 2 bits
    pub cell_reselection_types_supported: u8,
    /// Type1, 1 bit
    pub neighbour_cell_synchronized: bool,
    /// Type1, 2 bits
    pub cell_load_ca: u8,
    /// Type1, 12 bits
    pub main_carrier_number: u16,
    /// Type2, 10 bits
    pub main_carrier_number_extension: Option<u16>,
    /// Type2, 10 bits
    pub mcc: Option<u16>,
    /// Type2, 14 bits
    pub mnc: Option<u16>,
    /// Type2, 14 bits
    pub location_area: Option<u16>,
    /// Type2, 3 bits
    pub maximum_ms_transmit_power: Option<u8>,
    /// Type2, 4 bits
    pub minimum_rx_access_level: Option<u8>,
    /// Type2, 16 bits
    pub subscriber_class: Option<u16>,
    /// Type2, 12 bits
    pub bs_service_details: Option<BsServiceDetails>,
    /// Type2, 5 bits
    pub timeshare_cell_information_or_security_parameters: Option<u8>,
    /// Type2, 6 bits
    pub tdma_frame_offset: Option<u8>,
}

impl NeighbourCellInformationForCa {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let cell_identifier_ca = buf.read_field(5, "cell_identifier_ca")? as u8;
        let cell_reselection_types_supported = buf.read_field(2, "cell_reselection_types_supported")? as u8;
        let neighbour_cell_synchronized = buf.read_field(1, "neighbour_cell_synchronized")? != 0;
        let cell_load_ca = buf.read_field(2, "cell_load_ca")? as u8;
        let main_carrier_number = buf.read_field(12, "main_carrier_number")? as u16;

        // O-bit: indicates presence of any optional Type2 fields (ETSI 18.5.17 Table 18.64)
        let obit = delimiters::read_obit(buf)?;

        // For this element, each optional Type2 field is preceded by a P-bit.
        let main_carrier_number_extension =
            typed::parse_type2_generic(obit, buf, 10, "main_carrier_number_extension")?.map(|v| v as u16);
        let mcc = typed::parse_type2_generic(obit, buf, 10, "mcc")?.map(|v| v as u16);
        let mnc = typed::parse_type2_generic(obit, buf, 14, "mnc")?.map(|v| v as u16);
        let location_area = typed::parse_type2_generic(obit, buf, 14, "location_area")?.map(|v| v as u16);
        let maximum_ms_transmit_power =
            typed::parse_type2_generic(obit, buf, 3, "maximum_ms_transmit_power")?.map(|v| v as u8);
        let minimum_rx_access_level =
            typed::parse_type2_generic(obit, buf, 4, "minimum_rx_access_level")?.map(|v| v as u8);
        let subscriber_class = typed::parse_type2_generic(obit, buf, 16, "subscriber_class")?.map(|v| v as u16);
        let bs_service_details = typed::parse_type2_struct(obit, buf, BsServiceDetails::from_bitbuf)?;
        let timeshare_cell_information_or_security_parameters =
            typed::parse_type2_generic(obit, buf, 5, "timeshare_cell_information_or_security_parameters")?.map(|v| v as u8);
        let tdma_frame_offset = typed::parse_type2_generic(obit, buf, 6, "tdma_frame_offset")?.map(|v| v as u8);

        Ok(NeighbourCellInformationForCa {
            cell_identifier_ca,
            cell_reselection_types_supported,
            neighbour_cell_synchronized,
            cell_load_ca,
            main_carrier_number,
            main_carrier_number_extension,
            mcc,
            mnc,
            location_area,
            maximum_ms_transmit_power,
            minimum_rx_access_level,
            subscriber_class,
            bs_service_details,
            timeshare_cell_information_or_security_parameters,
            tdma_frame_offset,
        })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.ensure_bits(self.cell_identifier_ca as u64, 5, "cell_identifier_ca")?;
        self.ensure_bits(
            self.cell_reselection_types_supported as u64,
            2,
            "cell_reselection_types_supported",
        )?;
        self.ensure_bits(self.cell_load_ca as u64, 2, "cell_load_ca")?;
        self.ensure_bits(self.main_carrier_number as u64, 12, "main_carrier_number")?;
        self.ensure_opt_bits(
            self.main_carrier_number_extension.map(|v| v as u64),
            10,
            "main_carrier_number_extension",
        )?;
        self.ensure_opt_bits(self.mcc.map(|v| v as u64), 10, "mcc")?;
        self.ensure_opt_bits(self.mnc.map(|v| v as u64), 14, "mnc")?;
        self.ensure_opt_bits(self.location_area.map(|v| v as u64), 14, "location_area")?;
        self.ensure_opt_bits(
            self.maximum_ms_transmit_power.map(|v| v as u64),
            3,
            "maximum_ms_transmit_power",
        )?;
        self.ensure_opt_bits(
            self.minimum_rx_access_level.map(|v| v as u64),
            4,
            "minimum_rx_access_level",
        )?;
        self.ensure_opt_bits(self.subscriber_class.map(|v| v as u64), 16, "subscriber_class")?;
        self.ensure_opt_bits(
            self.timeshare_cell_information_or_security_parameters.map(|v| v as u64),
            5,
            "timeshare_cell_information_or_security_parameters",
        )?;
        self.ensure_opt_bits(self.tdma_frame_offset.map(|v| v as u64), 6, "tdma_frame_offset")?;

        // Mandatory fields (22 bits total)
        buf.write_bits(self.cell_identifier_ca as u64, 5);
        buf.write_bits(self.cell_reselection_types_supported as u64, 2);
        buf.write_bits(self.neighbour_cell_synchronized as u64, 1);
        buf.write_bits(self.cell_load_ca as u64, 2);
        buf.write_bits(self.main_carrier_number as u64, 12);

        // O-bit: 1 if any optional field is present, 0 otherwise (ETSI 18.5.17 Table 18.64)
        let obit = self.main_carrier_number_extension.is_some()
            || self.mcc.is_some()
            || self.mnc.is_some()
            || self.location_area.is_some()
            || self.maximum_ms_transmit_power.is_some()
            || self.minimum_rx_access_level.is_some()
            || self.subscriber_class.is_some()
            || self.bs_service_details.is_some()
            || self.timeshare_cell_information_or_security_parameters.is_some()
            || self.tdma_frame_offset.is_some();
        delimiters::write_obit(buf, obit as u8);

        // Optional fields — each preceded by a P-bit (only written when obit=true)
        typed::write_type2_generic(obit, buf, self.main_carrier_number_extension.map(|v| v as u64), 10);
        typed::write_type2_generic(obit, buf, self.mcc.map(|v| v as u64), 10);
        typed::write_type2_generic(obit, buf, self.mnc.map(|v| v as u64), 14);
        typed::write_type2_generic(obit, buf, self.location_area.map(|v| v as u64), 14);
        typed::write_type2_generic(obit, buf, self.maximum_ms_transmit_power.map(|v| v as u64), 3);
        typed::write_type2_generic(obit, buf, self.minimum_rx_access_level.map(|v| v as u64), 4);
        typed::write_type2_generic(obit, buf, self.subscriber_class.map(|v| v as u64), 16);
        typed::write_type2_struct(obit, buf, &self.bs_service_details, |val, inner| {
            val.to_bitbuf(inner);
            Ok(())
        })?;
        typed::write_type2_generic(obit, buf, self.timeshare_cell_information_or_security_parameters.map(|v| v as u64), 5);
        typed::write_type2_generic(obit, buf, self.tdma_frame_offset.map(|v| v as u64), 6);

        Ok(())
    }

    fn ensure_bits(&self, value: u64, bits: u8, field: &'static str) -> Result<(), PduParseErr> {
        let max = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
        if value > max {
            return Err(PduParseErr::InvalidValue { field, value });
        }
        Ok(())
    }

    fn ensure_opt_bits(&self, value: Option<u64>, bits: u8, field: &'static str) -> Result<(), PduParseErr> {
        if let Some(v) = value {
            self.ensure_bits(v, bits, field)?;
        }
        Ok(())
    }
}

impl fmt::Display for NeighbourCellInformationForCa {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "NeighbourCellInformationForCa {{ cell_identifier_ca: {:?} cell_reselection_types_supported: {:?} neighbour_cell_synchronized: {:?} cell_load_ca: {:?} main_carrier_number: {:?} main_carrier_number_extension: {:?} mcc: {:?} mnc: {:?} location_area: {:?} maximum_ms_transmit_power: {:?} minimum_rx_access_level: {:?} subscriber_class: {:?} bs_service_details: {:?} timeshare_cell_information_or_security_parameters: {:?} tdma_frame_offset: {:?} }}",
            self.cell_identifier_ca,
            self.cell_reselection_types_supported,
            self.neighbour_cell_synchronized,
            self.cell_load_ca,
            self.main_carrier_number,
            self.main_carrier_number_extension,
            self.mcc,
            self.mnc,
            self.location_area,
            self.maximum_ms_transmit_power,
            self.minimum_rx_access_level,
            self.subscriber_class,
            self.bs_service_details,
            self.timeshare_cell_information_or_security_parameters,
            self.tdma_frame_offset,
        )
    }
}
