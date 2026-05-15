use tetra_core::{BitBuffer, expect_value, pdu_parse_error::PduParseErr, typed_pdu_fields::delimiters};

/// EN 300 392-9, table 5: Supplementary service type for TPI.
pub const SS_TYPE_TPI: u8 = 3;
/// EN 300 392-12-3, table 33: TPI INFORM PDU type.
pub const TPI_PDU_TYPE_INFORM: u8 = 0b10001;
/// EN 300 392-2 SDS text coding value also used by FlowStation for 8-bit ASCII/ISO-8859-1 text.
pub const TEXT_ENCODING_8BIT: u8 = 0x01;
pub const MAX_MNEMONIC_NAME_CHARS: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsTpiInform {
    pub clir_invoked: bool,
    pub mnemonic_name: Option<String>,
    pub talking_sending_party_ssi: Option<u32>,
    pub tx_demand_priority: Option<u8>,
}

impl SsTpiInform {
    pub fn for_ssi(ssi: u32, mnemonic_name: Option<String>) -> Self {
        Self {
            clir_invoked: false,
            mnemonic_name,
            talking_sending_party_ssi: Some(ssi),
            tx_demand_priority: None,
        }
    }

    pub fn clir() -> Self {
        Self {
            clir_invoked: true,
            mnemonic_name: None,
            talking_sending_party_ssi: None,
            tx_demand_priority: None,
        }
    }

    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let ss_type = buffer.read_field(6, "ss_type")? as u8;
        expect_value!(ss_type, SS_TYPE_TPI, "ss_type")?;

        let tpi_pdu_type = buffer.read_field(5, "tpi_pdu_type")? as u8;
        expect_value!(tpi_pdu_type, TPI_PDU_TYPE_INFORM, "tpi_pdu_type")?;

        let clir_invoked = buffer.read_field(1, "clir_invoked")? != 0;

        let mnemonic_name = if clir_invoked {
            None
        } else {
            let text_encoding_scheme = buffer.read_field(7, "text_encoding_scheme")? as u8;
            if text_encoding_scheme != TEXT_ENCODING_8BIT {
                return Err(PduParseErr::NotImplemented {
                    field: Some("text_encoding_scheme"),
                });
            }

            let name_len_bits = buffer.read_field(8, "mnemonic_name_len_bits")? as usize;
            if name_len_bits % 8 != 0 || name_len_bits / 8 > MAX_MNEMONIC_NAME_CHARS {
                return Err(PduParseErr::InvalidValue {
                    field: "mnemonic_name_len_bits",
                    value: name_len_bits as u64,
                });
            }

            let mut bytes = Vec::with_capacity(name_len_bits / 8);
            for _ in 0..name_len_bits / 8 {
                bytes.push(buffer.read_field(8, "mnemonic_name_char")? as u8);
            }

            if bytes.is_empty() {
                None
            } else {
                Some(bytes.iter().map(|byte| *byte as char).collect())
            }
        };

        let obit = delimiters::read_obit(buffer)?;
        let address_type = if obit {
            match delimiters::read_pbit(buffer)? {
                true => Some(buffer.read_field(2, "talking_sending_party_address_type")?),
                false => None,
            }
        } else {
            None
        };

        let talking_sending_party_ssi = match address_type {
            Some(1) => Some(buffer.read_field(24, "talking_sending_party_ssi")? as u32),
            Some(2) => {
                let ssi = buffer.read_field(24, "talking_sending_party_ssi")? as u32;
                let _extension = buffer.read_field(24, "talking_sending_party_extension")?;
                Some(ssi)
            }
            Some(value) => {
                return Err(PduParseErr::InvalidValue {
                    field: "talking_sending_party_address_type",
                    value,
                });
            }
            None => None,
        };

        let tx_demand_priority = if obit {
            match delimiters::read_pbit(buffer)? {
                true => Some(buffer.read_field(2, "tx_demand_priority")? as u8),
                false => None,
            }
        } else {
            None
        };

        Ok(Self {
            clir_invoked,
            mnemonic_name,
            talking_sending_party_ssi,
            tx_demand_priority,
        })
    }

    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        buffer.write_bits(SS_TYPE_TPI as u64, 6);
        buffer.write_bits(TPI_PDU_TYPE_INFORM as u64, 5);
        buffer.write_bits(self.clir_invoked as u64, 1);

        if self.clir_invoked {
            delimiters::write_obit(buffer, 0);
            return Ok(());
        }

        let mnemonic = self.mnemonic_name.as_deref().unwrap_or("");
        let bytes = mnemonic.as_bytes();
        if bytes.len() > MAX_MNEMONIC_NAME_CHARS {
            return Err(PduParseErr::InvalidValue {
                field: "mnemonic_name",
                value: bytes.len() as u64,
            });
        }
        if !bytes.is_ascii() {
            return Err(PduParseErr::NotImplemented {
                field: Some("mnemonic_name_non_ascii"),
            });
        }

        buffer.write_bits(TEXT_ENCODING_8BIT as u64, 7);
        buffer.write_bits((bytes.len() * 8) as u64, 8);
        for byte in bytes {
            buffer.write_bits(*byte as u64, 8);
        }

        let obit = self.talking_sending_party_ssi.is_some() || self.tx_demand_priority.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        match self.talking_sending_party_ssi {
            Some(ssi) => {
                delimiters::write_pbit(buffer, 1);
                buffer.write_bits(1, 2);
                buffer.write_bits(ssi as u64, 24);
            }
            None => delimiters::write_pbit(buffer, 0),
        }

        match self.tx_demand_priority {
            Some(priority) if priority <= 3 => {
                delimiters::write_pbit(buffer, 1);
                buffer.write_bits(priority as u64, 2);
            }
            Some(priority) => {
                return Err(PduParseErr::InvalidValue {
                    field: "tx_demand_priority",
                    value: priority as u64,
                });
            }
            None => delimiters::write_pbit(buffer, 0),
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_core::typed_pdu_fields::typed;

    #[test]
    fn ss_tpi_inform_identity_and_mnemonic_roundtrip() {
        let inform = SsTpiInform::for_ssi(2_260_571, Some("YO6RZV".to_string()));
        let mut buffer = BitBuffer::new_autoexpand(160);
        typed::write_type3_struct(true, &mut buffer, &Some(inform.clone()), 3u64, SsTpiInform::to_bitbuf).unwrap();
        buffer.write_bits(0, 1);
        buffer.seek(0);

        let parsed = typed::parse_type3_struct(true, &mut buffer, 3u64, SsTpiInform::from_bitbuf).unwrap();
        assert_eq!(parsed, Some(inform));
        assert_eq!(buffer.read_field(1, "trailing_mbit").unwrap(), 0);
    }

    #[test]
    fn ss_tpi_inform_identity_only_roundtrip() {
        let inform = SsTpiInform::for_ssi(1234, None);
        let mut buffer = BitBuffer::new_autoexpand(128);
        inform.to_bitbuf(&mut buffer).unwrap();
        buffer.seek(0);

        let parsed = SsTpiInform::from_bitbuf(&mut buffer).unwrap();
        assert_eq!(parsed, inform);
    }

    #[test]
    fn ss_tpi_inform_clir_roundtrip() {
        let inform = SsTpiInform::clir();
        let mut buffer = BitBuffer::new_autoexpand(32);
        inform.to_bitbuf(&mut buffer).unwrap();
        buffer.seek(0);

        let parsed = SsTpiInform::from_bitbuf(&mut buffer).unwrap();
        assert_eq!(parsed, inform);
    }

    #[test]
    fn ss_tpi_rejects_mnemonic_longer_than_15_chars() {
        let inform = SsTpiInform::for_ssi(1234, Some("ABCDEFGHIJKLMNOP".to_string()));
        let mut buffer = BitBuffer::new_autoexpand(128);
        assert!(inform.to_bitbuf(&mut buffer).is_err());
    }
}
