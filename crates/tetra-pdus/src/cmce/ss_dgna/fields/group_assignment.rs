use core::fmt;

use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::cmce::ss_dgna::enums::results::GroupIdentityAttachmentMode;

/// Group assignment IE, TS 100 392-12-22 V1.5.1 Table 45.
///
/// This is the per-group definition carried (repeated) in the ASSIGN PDU
/// (Table 18). It is the superset of the MM group-attach element: in addition
/// to the GSSI and attachment mode it carries the mnemonic display name and the
/// security / additional-group fields the MS needs to materialise and name a
/// dynamic talkgroup.
///
/// Layout:
/// ```text
///   Group SSI                          24b  M
///   Group extension present             1b  M
///   Group extension                    24b  C  (only if present = 1; CC 10b + NC 14b, Table 49)
///   Group identity attachment mode      3b  M  (Table 51)
///   --- O-bit (Annex E) gating the type-2 region ---
///   Class of usage                      3b  O/t2 (P-bit; EN 300 392-2 cl.16.10.6)
///   Mnemonic group name                var  O/t2 (P-bit; TETRA text string, EN 300 392-9 Table 17)
///   Length of security related info     6b  O/t2 (P-bit) ─┐ paired length + value
///   Security related information       var  C            ─┘
///   Length of additional group info     6b  O/t2 (P-bit) ─┐ paired length + value
///   Additional group information        var  C            ─┘
///   (V)GSSI                            24b  O/t2 (P-bit)
/// ```
///
/// Type-2 framing follows EN 300 392-2 V2.4.1 Annex E: a single O-bit after the
/// fixed (type-1) region signals that any optional element may follow, and each
/// defined type-2 element is then preceded by its own P-bit, written in the
/// table order above. When every optional is absent the O-bit is 0 and no
/// P-bits are emitted.
///
/// Mnemonic character encoding: TS 100 392-12-22 points at the EN 300 392-9
/// text-string format. FlowStation emits the common vendor shape
/// coding-scheme=0x01 (ISO-8859-1 / Latin-1), 8-bit length-in-bits, then the
/// text octets. Decode accepts both 0x01 (Latin-1) and 0x1A (UTF-16BE/UCS-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupAssignment {
    /// Group SSI (GSSI), 24 bits.
    pub group_ssi: u32,
    /// Group extension, 24 bits (CC 10b + NC 14b). `Some` sets the
    /// "Group extension present" bit; `None` clears it.
    pub group_extension: Option<u32>,
    /// Group identity attachment mode, 3 bits (Table 51).
    pub attachment_mode: GroupIdentityAttachmentMode,
    /// Class of usage, 3 bits (type-2). Reuses the EN 300 392-2 cl.16.10.6
    /// value table; absent when the attachment mode does not carry it.
    pub class_of_usage: Option<u8>,
    /// Mnemonic group name (type-2), the alias the radio displays. Stored as
    /// Unicode text without the on-air coding-scheme/length wrapper; `None` = absent.
    pub mnemonic: Option<String>,
    /// Security related information (type-2), opaque octets. `None` = absent.
    pub security_related_information: Option<Vec<u8>>,
    /// Additional group information (type-2), opaque octets. `None` = absent.
    pub additional_group_information: Option<Vec<u8>>,
    /// (V)GSSI, 24 bits (type-2). `None` = absent.
    pub vgssi: Option<u32>,
}

impl GroupAssignment {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        // Fixed (type-1) region.
        let group_ssi = buf.read_field(24, "group_ssi")? as u32;
        let ext_present = buf.read_field(1, "group_extension_present")? == 1;
        let group_extension = if ext_present {
            Some(buf.read_field(24, "group_extension")? as u32)
        } else {
            None
        };
        let attachment_mode_raw = buf.read_field(3, "attachment_mode")?;
        let attachment_mode = GroupIdentityAttachmentMode::try_from(attachment_mode_raw).map_err(|_| PduParseErr::InvalidValue {
            field: "attachment_mode",
            value: attachment_mode_raw,
        })?;

        // O-bit gating the type-2 region (Annex E). If clear, every optional is
        // absent and no P-bits follow.
        let obit = delimiters::read_obit(buf)?;

        // Class of usage (type-2), 3b.
        let class_of_usage = typed::parse_type2_generic(obit, buf, 3, "class_of_usage")?.map(|v| v as u8);

        // Mnemonic group name (type-2): P-bit, then EN 300 392-9 Table-17 text string.
        let mnemonic = Self::parse_type2_mnemonic(obit, buf)?;

        // Security related information (type-2): same length+value shape.
        let security_related_information = Self::parse_type2_octets(obit, buf, "security_related_information")?;

        // Additional group information (type-2): same length+value shape.
        let additional_group_information = Self::parse_type2_octets(obit, buf, "additional_group_information")?;

        // (V)GSSI (type-2), 24b.
        let vgssi = typed::parse_type2_generic(obit, buf, 24, "vgssi")?.map(|v| v as u32);

        Ok(GroupAssignment {
            group_ssi,
            group_extension,
            attachment_mode,
            class_of_usage,
            mnemonic,
            security_related_information,
            additional_group_information,
            vgssi,
        })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        if let Some(cou) = self.class_of_usage {
            if cou > 0b111 {
                return Err(PduParseErr::InvalidValue {
                    field: "class_of_usage",
                    value: cou as u64,
                });
            }
        }

        // Fixed (type-1) region.
        buf.write_bits(self.group_ssi as u64, 24);
        buf.write_bits(self.group_extension.is_some() as u64, 1);
        if let Some(ext) = self.group_extension {
            buf.write_bits(ext as u64, 24);
        }
        buf.write_bits(self.attachment_mode.into_raw(), 3);

        // O-bit: set if any type-2 optional is present.
        let obit = self.class_of_usage.is_some()
            || self.mnemonic.is_some()
            || self.security_related_information.is_some()
            || self.additional_group_information.is_some()
            || self.vgssi.is_some();
        delimiters::write_obit(buf, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type-2 elements, each preceded by its P-bit, in Table 45 order.
        typed::write_type2_generic(obit, buf, self.class_of_usage.map(|v| v as u64), 3);
        Self::write_type2_mnemonic(obit, buf, &self.mnemonic)?;
        Self::write_type2_octets(obit, buf, &self.security_related_information)?;
        Self::write_type2_octets(obit, buf, &self.additional_group_information)?;
        typed::write_type2_generic(obit, buf, self.vgssi.map(|v| v as u64), 24);

        Ok(())
    }

    /// Parse a type-2 element whose value is a 6-bit octet count followed by
    /// that many octets (mnemonic / security / additional-group fields).
    fn parse_type2_octets(obit: bool, buf: &mut BitBuffer, field: &'static str) -> Result<Option<Vec<u8>>, PduParseErr> {
        if !obit {
            return Ok(None);
        }
        if !delimiters::read_pbit(buf)? {
            return Ok(None);
        }
        let octet_count = buf.read_field(6, field)? as usize;
        let mut bytes = Vec::with_capacity(octet_count);
        for _ in 0..octet_count {
            bytes.push(buf.read_field(8, field)? as u8);
        }
        Ok(Some(bytes))
    }

    /// Write a type-2 element as P-bit, 6-bit octet count, then the octets.
    fn write_type2_octets(obit: bool, buf: &mut BitBuffer, value: &Option<Vec<u8>>) -> Result<(), PduParseErr> {
        if !obit {
            assert!(value.is_none(), "Type2 element cannot be present when obit is false");
            return Ok(());
        }
        match value {
            Some(bytes) => {
                if bytes.len() > 0b11_1111 {
                    return Err(PduParseErr::InvalidValue {
                        field: "type2_octets_length",
                        value: bytes.len() as u64,
                    });
                }
                delimiters::write_pbit(buf, 1);
                buf.write_bits(bytes.len() as u64, 6);
                for b in bytes {
                    buf.write_bits(*b as u64, 8);
                }
            }
            None => {
                delimiters::write_pbit(buf, 0);
            }
        }
        Ok(())
    }

    fn parse_type2_mnemonic(obit: bool, buf: &mut BitBuffer) -> Result<Option<String>, PduParseErr> {
        if !obit {
            return Ok(None);
        }
        if !delimiters::read_pbit(buf)? {
            return Ok(None);
        }

        let coding_scheme = buf.read_field(7, "mnemonic_coding_scheme")? as u8;
        let text_len_bits = buf.read_field(8, "mnemonic_length_bits")? as usize;
        let mnemonic = match coding_scheme {
            0x01 => {
                if text_len_bits % 8 != 0 {
                    return Err(PduParseErr::InvalidValue {
                        field: "mnemonic_length_bits",
                        value: text_len_bits as u64,
                    });
                }
                let mut out = String::with_capacity(text_len_bits / 8);
                for _ in 0..(text_len_bits / 8) {
                    out.push(buf.read_field(8, "mnemonic_char")? as u8 as char);
                }
                out
            }
            0x1A => {
                if text_len_bits % 16 != 0 {
                    return Err(PduParseErr::InvalidValue {
                        field: "mnemonic_length_bits",
                        value: text_len_bits as u64,
                    });
                }
                let mut units = Vec::with_capacity(text_len_bits / 16);
                for _ in 0..(text_len_bits / 16) {
                    units.push(buf.read_field(16, "mnemonic_char")? as u16);
                }
                char::decode_utf16(units)
                    .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect()
            }
            _ => {
                return Err(PduParseErr::InvalidValue {
                    field: "mnemonic_coding_scheme",
                    value: coding_scheme as u64,
                });
            }
        };
        Ok(Some(mnemonic))
    }

    fn write_type2_mnemonic(obit: bool, buf: &mut BitBuffer, value: &Option<String>) -> Result<(), PduParseErr> {
        if !obit {
            assert!(value.is_none(), "Type2 element cannot be present when obit is false");
            return Ok(());
        }

        match value {
            Some(text) => {
                let char_count = text.chars().count();
                if !(1..=15).contains(&char_count) {
                    return Err(PduParseErr::InvalidValue {
                        field: "mnemonic_char_count",
                        value: char_count as u64,
                    });
                }

                let mut bytes = Vec::with_capacity(char_count);
                for ch in text.chars() {
                    let cp = ch as u32;
                    bytes.push(if cp <= 0xFF { cp as u8 } else { b'?' });
                }
                let text_len_bits = bytes.len() * 8;
                delimiters::write_pbit(buf, 1);
                // EN 300 392-9 clause 8.4.2 / table 17: text coding scheme is 7 bits.
                buf.write_bits(0x01, 7);
                buf.write_bits(text_len_bits as u64, 8);
                for b in bytes {
                    buf.write_bits(b as u64, 8);
                }
            }
            None => delimiters::write_pbit(buf, 0),
        }
        Ok(())
    }
}

impl fmt::Display for GroupAssignment {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "GroupAssignment {{ group_ssi: {} group_extension: {:?} attachment_mode: {} class_of_usage: {:?} mnemonic: {:?} security_related_information: {:?} additional_group_information: {:?} vgssi: {:?} }}",
            self.group_ssi,
            self.group_extension,
            self.attachment_mode,
            self.class_of_usage,
            self.mnemonic,
            self.security_related_information,
            self.additional_group_information,
            self.vgssi
        )
    }
}
