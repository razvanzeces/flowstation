use crate::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};

#[derive(Debug, Clone)]
pub struct CmceChanAllocReq {
    /// Set for new allocation, None for QuitAndGo
    pub usage: Option<u8>,
    /// Carrier number; by default, uses the scheduler's serving carrier.
    pub carrier: Option<u16>,
    /// Bitmap of slots to use.
    pub timeslots: [bool; 4],
    /// Alloc type.
    /// Additional: new allocation.
    /// Replace: update existing allocation, or create if it does not exist.
    /// QuitAndGo: remove existing allocation.
    pub alloc_type: ChanAllocType,
    pub ul_dl_assigned: UlDlAssignment,
    /// The channel is on another cell (announced type 1 cell reselection, TS 100 392-2 clause
    /// 21.5.2 "cell change flag"): `carrier` and `timeslots` are the new cell's.
    pub cell_change: bool,
}
