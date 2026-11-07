//! Site switch: makes the cells of a multi-cell station look like one stack to the network
//! entity (Brew or LST Dispatch).
//!
//! Every cell's radio stack runs on its own thread with its own router. The network entity lives
//! only in the primary router, wrapped by [`SiteSwitch`] in the `TetraEntity::Brew` slot. Each
//! additional cell has a [`CellLink`] in its Brew slot that forwards everything to the switch.
//!
//! Once cells are linked (`StackConfig::is_site_linked`) they report every registration, group
//! floor and call to their network slot; the switch applies the Brew routing rules before
//! anything reaches the real network entity. The switch:
//! * tracks where radios are registered and which cells have members of each group;
//! * routes network → stack messages to the right cell: by carrier (carriers are unique per
//!   cell), by Brew session UUID, by destination ISSI/GSSI location, else to the primary;
//! * fans a network group call out to every cell with members, forwards only the first
//!   `NetworkCallReady` to the network entity and copies its downlink voice to the other cells;
//! * lets a radio's group call be heard on the other cells: they get it as a network call and
//!   the talker's uplink voice is copied to them; one talker per group site-wide (a radio that
//!   takes the floor while another cell's radio talks is pulled into that call instead);
//! * connects individual calls between radios on different cells by passing the circuit-call
//!   signalling across (each CMCE sees the other as the network) and copying the voice;
//! * delivers SDS between cells directly, renumbers call identifiers, and mirrors the network
//!   link state from the primary to the other cells.
//!
//! [`SiteRelay`] does the same for the Asterisk SIP bridge (the primary's `Asterisk` slot), so
//! radios on every cell can call and be called through the PBX.

mod directory;
mod local;
mod relay;
mod switch;

use std::sync::OnceLock;

use tetra_config::bluestation::SharedConfig;
use tetra_core::CellId;

pub use directory::{SharedDirectory, SiteDirectory};
pub use local::LocalNetwork;
pub use relay::SiteRelay;
pub use switch::{CellLink, RelayPorts, SitePorts, SiteSwitch, site_links};

static EXTRA_CELLS: OnceLock<Vec<(CellId, SharedConfig)>> = OnceLock::new();

/// Record the running additional cells' configs (set once at startup) so the dashboard can show
/// every cell's registrations and settings.
pub fn register_extra_cells(cells: Vec<(CellId, SharedConfig)>) {
    let _ = EXTRA_CELLS.set(cells);
}

/// The running additional cells (empty for a single-cell station).
pub fn extra_cells() -> &'static [(CellId, SharedConfig)] {
    EXTRA_CELLS.get().map(|v| v.as_slice()).unwrap_or(&[])
}
