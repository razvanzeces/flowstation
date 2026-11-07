//! The network entity of a network with no core: nothing leaves the station.
//!
//! A `[networks.<name>]` entry without a `brew` link is switched inside the station. Its cells
//! still report registrations, floors and calls to their `TetraEntity::Brew` slot, where the
//! site switch joins the cells; whatever the switch would hand on to a core ends here.

use tetra_core::tetra_entities::TetraEntity;
use tetra_saps::SapMsg;

use crate::{MessageQueue, TetraEntityTrait};

/// Sink in the Brew slot of a local network's home cell.
pub struct LocalNetwork {
    name: String,
}

impl LocalNetwork {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl TetraEntityTrait for LocalNetwork {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Brew
    }

    fn rx_prim(&mut self, _queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("local network {}: no core, dropping {:?}", self.name, message.msg);
    }
}
