use core::fmt;

use tetra_core::pdu_parse_error::PduParseErr;
use tetra_core::{BitBuffer, expect_failed, expect_value};

use crate::umac::enums::reservation_requirement::ReservationRequirement;

/// Clause 21.4.2.5 MAC-END (uplink)
#[derive(Debug, Clone)]
pub struct MacEndUl {
    // 1
    pub fill_bits: bool,
    // 6
    // If 2-bits length_ind_cap_req < 0b11, field holds 6-bit length indication
    pub length_ind: Option<u8>,
    // If 2-bits length_ind_cap_req == 0b11, then reservation_req field holds 4  data bits
    pub reservation_req: Option<ReservationRequirement>,
}

impl MacEndUl {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        // required constant mac_pdu_type
        let mac_pdu_type = buf.read_field(2, "mac_pdu_type")?;
        assert!(mac_pdu_type == 1);
        // required constant pdu_subtype
        let pdu_subtype = buf.read_field(1, "pdu_subtype")?;
        assert!(pdu_subtype == 1);
        let fill_bits = buf.read_field(1, "fill_bits")? != 0;
        let length_ind_cap_req = buf.read_field(6, "length_ind_cap_req")?;
        let (length_ind, reservation_req) = if length_ind_cap_req == 0 {
            // Reserved value
            return expect_failed!(length_ind_cap_req, "length_ind_cap_req reserved value");
        } else if length_ind_cap_req < 0b101111 {
            // Length indication
            (Some(length_ind_cap_req as u8), None)
        } else if length_ind_cap_req < 0b110000 {
            // reserved value (47), return error
            return expect_failed!(length_ind_cap_req, "length_ind_cap_req reserved value");
        } else {
            // 0x110000 or higher, cap req
            let val = length_ind_cap_req & 0b001111;
            let res_req = ReservationRequirement::try_from(val).map_err(|_| PduParseErr::InvalidValue {
                field: "reservation_req",
                value: val,
            })?;
            (None, Some(res_req))
        };

        Ok(MacEndUl {
            fill_bits,
            length_ind,
            reservation_req,
        })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        // write required constant mac_pdu_type
        buf.write_bits(1, 2);
        // write required constant pdu_subtype
        buf.write_bits(1, 1);
        buf.write_bits(self.fill_bits as u8 as u64, 1);
        expect_value!(
            self.length_ind.is_some() ^ self.reservation_req.is_some(),
            true,
            "length_ind xor reservation_req must be present"
        )?;
        if let Some(length_ind) = self.length_ind {
            expect_value!(length_ind > 0, true, "length_ind zero")?;
            expect_value!(length_ind < 0b101110, true, "length_ind over 0b101110")?;
            buf.write_bits(length_ind as u64, 6);
        } else if let Some(reservation_req) = self.reservation_req {
            // assert!(reservation_req < 0b001111);
            buf.write_bits(0b11, 2);
            buf.write_bits(reservation_req as u64, 4);
        }
        Ok(())
    }
}

impl fmt::Display for MacEndUl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MacEndUl {{ fill_bits: {}", self.fill_bits)?;
        if let Some(length_ind) = self.length_ind {
            write!(f, "  length_ind: {}", length_ind)?;
        }
        if let Some(reservation_req) = self.reservation_req {
            write!(f, "  reservation_req: {}", reservation_req)?;
        }
        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(pdu: &MacEndUl) -> MacEndUl {
        let mut buf = BitBuffer::new_autoexpand(16);
        pdu.to_bitbuf(&mut buf).expect("serialize failed");
        buf.seek(0);
        MacEndUl::from_bitbuf(&mut buf).expect("parse failed")
    }

    /// Regression: the decode ladder compared against `0x110000` (hex = 1_114_112)
    /// instead of `0b110000` (48). Since the field is only 6 bits (max 63), every
    /// value was `< 0x110000`, so the cap-request branch was dead code and every
    /// reservation requirement decoded as "reserved value". Verify all 16 round-trip.
    #[test]
    fn reservation_req_round_trips() {
        for raw in 0u64..16 {
            let res_req = ReservationRequirement::try_from(raw).unwrap();
            let pdu = MacEndUl {
                fill_bits: false,
                length_ind: None,
                reservation_req: Some(res_req),
            };
            let parsed = round_trip(&pdu);
            assert_eq!(parsed.reservation_req, Some(res_req), "raw={raw}");
            assert_eq!(parsed.length_ind, None, "raw={raw}");
        }
    }

    /// A length-indication MAC-END-UL must still decode as a length indication.
    #[test]
    fn length_ind_round_trips() {
        for li in 1u8..0b101110 {
            let pdu = MacEndUl {
                fill_bits: true,
                length_ind: Some(li),
                reservation_req: None,
            };
            let parsed = round_trip(&pdu);
            assert_eq!(parsed.length_ind, Some(li), "li={li}");
            assert_eq!(parsed.reservation_req, None, "li={li}");
        }
    }
}
