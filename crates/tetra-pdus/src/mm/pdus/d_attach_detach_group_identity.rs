use core::fmt;

use tetra_core::expect_pdu_type;
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use crate::mm::enums::type34_elem_id_dl::MmType34ElemIdDl;
use crate::mm::fields::group_identity_downlink::GroupIdentityDownlink;

/// Representation of the D-ATTACH/DETACH GROUP IDENTITY PDU (Clause 16.9.2.1).
/// The infrastructure sends this message to the MS to indicate attachment/detachment of group identities for the MS or to initiate a group report request or give a group report response.
/// Response expected: -/U-ATTACH/DETACH GROUP IDENTITY ACKNOWLEDGEMENT
/// Response to: -/U-ATTACH/DETACH GROUP IDENTITY (report request)

// note 1: The MS shall accept the type 3/4 information elements both in the numerical order as described in annex E and in the order shown in this table.
#[derive(Debug)]
pub struct DAttachDetachGroupIdentity {
    /// Type1, 1 bits, Group identity report
    pub group_identity_report: bool,
    /// Type1, 1 bits, Group identity acknowledgement request
    pub group_identity_acknowledgement_request: bool,
    /// Type1, 1 bits, Group identity attach/detach mode
    pub group_identity_attach_detach_mode: bool,
    /// Type3, See note,
    pub proprietary: Option<Type3FieldGeneric>,
    /// Type3, See note,
    pub group_report_response: Option<Type3FieldGeneric>,
    /// Type4, See note,
    pub group_identity_downlink: Option<Vec<GroupIdentityDownlink>>,
    /// Type4, See ETSI EN 300 392-7 [8] and note,
    pub group_identity_security_related_information: Option<Type4FieldGeneric>,
}

#[allow(unreachable_code)] // TODO FIXME review, finalize and remove this
impl DAttachDetachGroupIdentity {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(4, "pdu_type")?;
        expect_pdu_type!(pdu_type, MmPduTypeDl::DAttachDetachGroupIdentity)?;

        // Type1
        let group_identity_report = buffer.read_field(1, "group_identity_report")? != 0;
        // Type1
        let group_identity_acknowledgement_request = buffer.read_field(1, "group_identity_acknowledgement_request")? != 0;
        // Type1
        let group_identity_attach_detach_mode = buffer.read_field(1, "group_identity_attach_detach_mode")? != 0;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type3
        let proprietary = typed::parse_type3_generic(obit, buffer, MmType34ElemIdDl::Proprietary)?;

        // Type3
        let group_report_response = typed::parse_type3_generic(obit, buffer, MmType34ElemIdDl::GroupReportResponse)?;

        // Type4
        let group_identity_downlink = typed::parse_type4_struct(
            obit,
            buffer,
            MmType34ElemIdDl::GroupIdentityDownlink,
            GroupIdentityDownlink::from_bitbuf,
        )?;

        // Type4
        let group_identity_security_related_information =
            typed::parse_type4_generic(obit, buffer, MmType34ElemIdDl::GroupIdentitySecurityRelatedInformation)?;

        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        Ok(DAttachDetachGroupIdentity {
            group_identity_report,
            group_identity_acknowledgement_request,
            group_identity_attach_detach_mode,
            proprietary,
            group_report_response,
            group_identity_downlink,
            group_identity_security_related_information,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        // PDU Type
        buffer.write_bits(MmPduTypeDl::DAttachDetachGroupIdentity.into_raw(), 4);
        // Type1
        buffer.write_bits(self.group_identity_report as u64, 1);
        // Type1
        buffer.write_bits(self.group_identity_acknowledgement_request as u64, 1);
        // Type1
        buffer.write_bits(self.group_identity_attach_detach_mode as u64, 1);

        // Check if any optional field present and place o-bit
        let obit = self.proprietary.is_some()
            || self.group_report_response.is_some()
            || self.group_identity_downlink.is_some()
            || self.group_identity_security_related_information.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type3
        typed::write_type3_generic(obit, buffer, &self.proprietary, MmType34ElemIdDl::Proprietary)?;

        // Type3
        typed::write_type3_generic(obit, buffer, &self.group_report_response, MmType34ElemIdDl::GroupReportResponse)?;

        // Type4
        typed::write_type4_struct(
            obit,
            buffer,
            &self.group_identity_downlink,
            MmType34ElemIdDl::GroupIdentityDownlink,
            GroupIdentityDownlink::to_bitbuf,
        )?;

        // Type4
        typed::write_type4_todo(
            obit,
            buffer,
            &self.group_identity_security_related_information,
            MmType34ElemIdDl::GroupIdentitySecurityRelatedInformation,
        )?;

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for DAttachDetachGroupIdentity {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DAttachDetachGroupIdentity {{ group_identity_report: {:?} group_identity_acknowledgement_request: {:?} group_identity_attach_detach_mode: {:?} proprietary: {:?} group_report_response: {:?} group_identity_downlink: {:?} group_identity_security_related_information: {:?} }}",
            self.group_identity_report,
            self.group_identity_acknowledgement_request,
            self.group_identity_attach_detach_mode,
            self.proprietary,
            self.group_report_response,
            self.group_identity_downlink,
            self.group_identity_security_related_information,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mm::fields::group_identity_attachment::GroupIdentityAttachment;

    /// A BS-initiated DGNA *attach* — one GSSI, persistent attachment (lifetime 0), ack requested,
    /// amend-mode (attach/detach_mode = false) — is exactly the shape FlowStation's DGNA send path
    /// emits. It must serialize and parse back to an identical PDU.
    #[test]
    fn dgna_attach_round_trips() {
        let pdu = DAttachDetachGroupIdentity {
            group_identity_report: false,
            group_identity_acknowledgement_request: true,
            group_identity_attach_detach_mode: false,
            proprietary: None,
            group_report_response: None,
            group_identity_downlink: Some(vec![GroupIdentityDownlink {
                group_identity_attachment: Some(GroupIdentityAttachment {
                    group_identity_attachment_lifetime: 0,
                    class_of_usage: 4,
                }),
                group_identity_detachment_uplink: None,
                gssi: Some(1234567),
                address_extension: None,
                vgssi: None,
            }]),
            group_identity_security_related_information: None,
        };

        let mut buf = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut buf).expect("serialize");
        buf.seek(0);
        let parsed = DAttachDetachGroupIdentity::from_bitbuf(&mut buf).expect("parse");

        assert!(!parsed.group_identity_report);
        assert!(parsed.group_identity_acknowledgement_request);
        assert!(
            !parsed.group_identity_attach_detach_mode,
            "DGNA must amend, not reset, the group list"
        );
        let gids = parsed.group_identity_downlink.expect("downlink present");
        assert_eq!(gids.len(), 1);
        assert_eq!(gids[0].gssi, Some(1234567));
        let att = gids[0].group_identity_attachment.as_ref().expect("attachment present");
        assert_eq!(att.group_identity_attachment_lifetime, 0, "lifetime 0 = persistent on MS");
        assert_eq!(att.class_of_usage, 4);
        assert!(gids[0].group_identity_detachment_uplink.is_none());
    }

    /// A BS-initiated DGNA *detach* — one GSSI carrying a detachment field instead of an
    /// attachment — must round-trip identically.
    #[test]
    fn dgna_detach_round_trips() {
        let pdu = DAttachDetachGroupIdentity {
            group_identity_report: false,
            group_identity_acknowledgement_request: true,
            group_identity_attach_detach_mode: false,
            proprietary: None,
            group_report_response: None,
            group_identity_downlink: Some(vec![GroupIdentityDownlink {
                group_identity_attachment: None,
                group_identity_detachment_uplink: Some(0),
                gssi: Some(7654321),
                address_extension: None,
                vgssi: None,
            }]),
            group_identity_security_related_information: None,
        };

        let mut buf = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut buf).expect("serialize");
        buf.seek(0);
        let parsed = DAttachDetachGroupIdentity::from_bitbuf(&mut buf).expect("parse");

        let gids = parsed.group_identity_downlink.expect("downlink present");
        assert_eq!(gids.len(), 1);
        assert_eq!(gids[0].gssi, Some(7654321));
        assert!(gids[0].group_identity_attachment.is_none(), "detach carries no attachment");
        assert_eq!(gids[0].group_identity_detachment_uplink, Some(0));
    }
}
