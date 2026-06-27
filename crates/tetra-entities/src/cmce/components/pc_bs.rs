use tetra_pdus::cmce::enums::cmce_pdu_type_ul::CmcePduTypeUl;
use tetra_saps::{SapMsg, SapMsgInner};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LcmcRoute {
    /// rd route: PC -> CC / CC -> PC, clause 14.2.6.
    CcRd,
    /// re route: PC -> SS / SS -> PC, clause 14.2.6.
    SsRe,
    /// rf route: PC -> SDS / SDS -> PC, clause 14.2.6.
    SdsRf,
    /// U-STATUS is an SDS PDU on rf, kept distinct because the BS implementation has a dedicated handler.
    SdsStatus,
    Unsupported(CmcePduTypeUl),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlRoute {
    /// ra/TNCC-side call-control input, including the local Brew/ISI bridge.
    CcRa,
    /// MM subscriber state used by CC for local routeing decisions.
    CcSubscriberUpdate,
    /// rc/TNSDS-side SDS input from the local network bridge.
    SdsRc,
    /// MM-requested SS-DGNA D-FACILITY emission, handled by the SS sub-entity.
    SsDgnaAssign,
    Unsupported,
}

/// BS-side Protocol Control role from EN 300 392-2 clause 14.2.5.
///
/// The standard defines PC as the router between CC/SS/SDS and LCMC. This component
/// keeps that discrimination out of the CC subentity so call control only receives
/// traffic that belongs on CC routes.
pub struct PcBs;

impl PcBs {
    pub fn new() -> Self {
        Self
    }

    pub fn route_lcmc_unitdata_ind(&self, message: &mut SapMsg) -> Option<LcmcRoute> {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::warn!("CMCE PC received non-LCMC unitdata indication: {:?}", message.msg);
            return None;
        };
        let Some(bits) = prim.sdu.peek_bits(5) else {
            tracing::warn!("CMCE PC received insufficient bits: {}", prim.sdu.dump_bin());
            return None;
        };
        let Ok(pdu_type) = CmcePduTypeUl::try_from(bits) else {
            tracing::warn!("CMCE PC received invalid UL PDU type {} in {}", bits, prim.sdu.dump_bin());
            return None;
        };

        Some(match pdu_type {
            CmcePduTypeUl::UAlert
            | CmcePduTypeUl::UCallRestore
            | CmcePduTypeUl::UConnect
            | CmcePduTypeUl::UDisconnect
            | CmcePduTypeUl::UInfo
            | CmcePduTypeUl::URelease
            | CmcePduTypeUl::USetup
            | CmcePduTypeUl::UTxCeased
            | CmcePduTypeUl::UTxDemand => LcmcRoute::CcRd,
            CmcePduTypeUl::UFacility => LcmcRoute::SsRe,
            CmcePduTypeUl::USdsData => LcmcRoute::SdsRf,
            CmcePduTypeUl::UStatus => LcmcRoute::SdsStatus,
            CmcePduTypeUl::CmceFunctionNotSupported => LcmcRoute::Unsupported(pdu_type),
        })
    }

    pub fn route_control(&self, message: &SapMsg) -> ControlRoute {
        match &message.msg {
            SapMsgInner::CmceCallControl(_) => ControlRoute::CcRa,
            SapMsgInner::MmSubscriberUpdate(_) => ControlRoute::CcSubscriberUpdate,
            SapMsgInner::CmceSdsData(_) => ControlRoute::SdsRc,
            SapMsgInner::CmceSsDgnaAssign { .. } => ControlRoute::SsDgnaAssign,
            _ => ControlRoute::Unsupported,
        }
    }
}
