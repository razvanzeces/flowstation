use core::fmt;

use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::mle::enums::mle_pdu_type_dl::MlePduTypeDl;
use crate::mle::fields::neighbour_cell_information_for_ca::NeighbourCellInformationForCa;

/// Representation of the D-NWRK-BROADCAST PDU (Clause 18.4.1.4.1).
/// Upon receipt from the SwMI, the message shall inform the MS-MLE about parameters for the CA serving cell and parameters for one or more CA neighbour cells.
/// Response expected: -
/// Response to: -/U-PREPARE/U-PREPARE-DA

// note 1: This element shall not be used by a DA MS.
// note 2: If present, the element shall indicate how many "Neighbour cell information for CA" elements follow. If not present, no neighbour cell information shall follow.
// note 3: The element definition is contained in clause 18.5 which gives the type and length for each sub-element which is included in this element. The element shall be present as many times as indicated by the "number of CA neighbour cells" element. There shall be no P-bit preceding each "neighbour cell information for CA" element which is carried by this PDU.
#[derive(Debug)]
pub struct DNwrkBroadcast {
    /// Type1, 16 bits, See note 1,
    pub cell_re_select_parameters: u16,
    /// Type1, 2 bits, See note 1,
    pub cell_load_ca: u8,
    /// Type2, 48 bits, TETRA network time
    pub tetra_network_time: Option<u64>,
    /// Type2, 3 bits, See note 2,
    pub number_of_ca_neighbour_cells: Option<u8>,
    /// Conditional See note 3, condition: number_of_ca_neighbour_cells > Some(0)
    pub neighbour_cell_information_for_ca: Vec<NeighbourCellInformationForCa>,
}

impl DNwrkBroadcast {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeDl::DNwrkBroadcast)?;

        // Type1
        let cell_re_select_parameters = buffer.read_field(16, "cell_re_select_parameters")? as u16;
        // Type1
        let cell_load_ca = buffer.read_field(2, "cell_load_ca")? as u8;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type2
        let tetra_network_time = typed::parse_type2_generic(obit, buffer, 48, "tetra_network_time")?;
        // Type2
        let number_of_ca_neighbour_cells =
            typed::parse_type2_generic(obit, buffer, 3, "number_of_ca_neighbour_cells")?.map(|v| v as u8);

        // Conditional: parse neighbour cell info elements
        let mut neighbour_cell_information_for_ca = Vec::new();
        if let Some(count) = number_of_ca_neighbour_cells {
            if count > 7 {
                return Err(PduParseErr::InvalidValue {
                    field: "number_of_ca_neighbour_cells",
                    value: count as u64,
                });
            }
            for _ in 0..count {
                neighbour_cell_information_for_ca.push(NeighbourCellInformationForCa::from_bitbuf(buffer)?);
            }
        }

        // Read trailing obit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        Ok(DNwrkBroadcast {
            cell_re_select_parameters,
            cell_load_ca,
            tetra_network_time,
            number_of_ca_neighbour_cells,
            neighbour_cell_information_for_ca,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        let neighbour_count = self.neighbour_cell_information_for_ca.len();

        // Validate consistency between count field and vec
        if let Some(count) = self.number_of_ca_neighbour_cells {
            if count as usize != neighbour_count {
                return Err(PduParseErr::Inconsistency {
                    field: "number_of_ca_neighbour_cells",
                    reason: "count does not match neighbour_cell_information_for_ca length",
                });
            }
            if count > 7 {
                return Err(PduParseErr::InvalidValue {
                    field: "number_of_ca_neighbour_cells",
                    value: count as u64,
                });
            }
        } else if neighbour_count > 0 {
            return Err(PduParseErr::Inconsistency {
                field: "neighbour_cell_information_for_ca",
                reason: "missing number_of_ca_neighbour_cells",
            });
        }

        // PDU Type
        buffer.write_bits(MlePduTypeDl::DNwrkBroadcast.into_raw(), 3);
        // Type1
        buffer.write_bits(self.cell_re_select_parameters as u64, 16);
        // Type1
        buffer.write_bits(self.cell_load_ca as u64, 2);

        // Check if any optional field present and place o-bit
        let obit = self.tetra_network_time.is_some() || self.number_of_ca_neighbour_cells.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_generic(obit, buffer, self.tetra_network_time, 48);

        // Type2
        typed::write_type2_generic(
            obit,
            buffer,
            self.number_of_ca_neighbour_cells.map(|v| v as u64),
            3,
        );

        // Conditional: write neighbour cell info elements (no P-bit per note 3)
        if self.number_of_ca_neighbour_cells.unwrap_or(0) > 0 {
            for neighbour in &self.neighbour_cell_information_for_ca {
                neighbour.to_bitbuf(buffer)?;
            }
        }

        // Write trailing obit=0 to signal end of optional fields.
        // from_bitbuf reads this bit; without it the MS reads a random bit from the
        // next PDU/padding and may interpret it as the start of another neighbour element.
        delimiters::write_obit(buffer, 0);

        Ok(())
    }
}

impl fmt::Display for DNwrkBroadcast {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DNwrkBroadcast {{ cell_re_select_parameters: {:?} cell_load_ca: {:?} tetra_network_time: {:?} number_of_ca_neighbour_cells: {:?} neighbour_cell_information_for_ca: {:?} }}",
            self.cell_re_select_parameters,
            self.cell_load_ca,
            self.tetra_network_time,
            self.number_of_ca_neighbour_cells,
            self.neighbour_cell_information_for_ca,
        )
    }
}
