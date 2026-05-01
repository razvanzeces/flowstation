use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// The three states a transmit receipt can be in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxState {
    /// Message is queued but not yet sent over the air.
    Pending = 0,
    /// MAC layer had to discard this message
    Discarded = 1,
    /// MAC layer has sent the PDU over the air.
    Transmitted = 2,
    /// Message was transmitted but acknowledgement never came
    Lost = 3,
    /// The remote side has acknowledged receipt.
    Acknowledged = 4,
}

impl TxState {
    fn from_raw(v: u8) -> Self {
        match v {
            0 => Self::Pending,
            1 => Self::Discarded,
            2 => Self::Transmitted,
            3 => Self::Lost,
            _ => Self::Acknowledged,
        }
    }
}

/// A transmit receipt kept by the originator (e.g. CMCE) to query whether the
/// message was sent and/or acknowledged.
///
/// State machine (transitions driven by the paired [`TxSignal`]):
///
/// ```text
/// Pending -> Transmitted | Discarded
///   Transmitted: MAC has sent the PDU over the air.
///   Discarded:   MAC was too busy. Final state.
///
/// expects_ack == true:
///   Transmitted -> Acknowledged | Lost
///     Acknowledged: LLC received ACK from remote. Final state.
///     Lost:         LLC timed out waiting for ACK. Final state.
///
/// expects_ack == false:
///   Transmitted is the final state.
/// ```

/// The reporting half of a transmit receipt, carried alongside the PDU down
/// through MAC and LLC. These layers call the `mark_*` methods to drive state
/// transitions that the paired [`TxReceipt`] can observe.
#[derive(Debug, Clone)]
pub struct TxReporter {
    expects_ack: bool,
    state: Arc<AtomicU8>,
    // t_tx: Option<TdmaTime>,
    // t_ack: Option<TdmaTime>
}

impl TxReporter {
    /// Creates a clonable TxReporter for acknowledged service. All clones share the same internal state.
    pub fn new() -> Self {
        let state = Arc::new(AtomicU8::new(TxState::Pending as u8));
        Self { expects_ack: true, state }
    }

    /// Creates a clonable TxReporter for unacknowledged service. All clones share the same internal state.
    pub fn new_unacked() -> Self {
        let mut ret = Self::new();
        ret.expects_ack = false;
        ret
    }

    /// Returns the current state.
    pub fn get_state(&self) -> TxState {
        TxState::from_raw(self.state.load(Ordering::Relaxed))
    }

    /// True if the PDU was discarded by the Umac due to congestion
    pub fn is_discarded(&self) -> bool {
        self.state.load(Ordering::Relaxed) == TxState::Discarded as u8
    }

    /// True once the PDU has been sent over the air (or further).
    pub fn is_transmitted(&self) -> bool {
        self.state.load(Ordering::Relaxed) >= TxState::Transmitted as u8
    }

    /// True once the remote side has acknowledged receipt.
    pub fn is_acknowledged(&self) -> bool {
        self.state.load(Ordering::Relaxed) >= TxState::Acknowledged as u8
    }

    /// Returns true if this is the final state for this message.
    pub fn is_in_final_state(&self) -> bool {
        match self.get_state() {
            TxState::Pending => false,
            TxState::Discarded => true,
            TxState::Transmitted => !self.expects_ack,
            TxState::Lost => true,
            TxState::Acknowledged => true,
        }
    }

    fn mark(&self, curr_state: TxState, new_state: TxState) {
        // tracing::info!("TxReporter: marking {:?} -> {:?}", curr_state, new_state);
        match self
            .state
            .compare_exchange(curr_state as u8, new_state as u8, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => {}
            Err(_) => {
                panic!(
                    "TxReporter: invalid transition {:?} -> {:?} (actual state: {:?})",
                    curr_state,
                    new_state,
                    self.get_state()
                );
            }
        }
    }

    fn mark_unchecked(&self, new_state: TxState) {
        self.state.store(new_state as u8, Ordering::Relaxed);
    }

    /// Pending → Transmitted: MAC layer has sent the PDU over the air.
    pub fn mark_transmitted(&self) {
        self.mark(TxState::Pending, TxState::Transmitted);
    }

    /// Pending → Discarded: MAC layer was too busy to transmit.
    pub fn mark_discarded(&self) {
        self.mark(TxState::Pending, TxState::Discarded);
    }

    /// Transmitted → Acknowledged: LLC received an ACK from the remote side.
    pub fn mark_acknowledged(&self) {
        assert!(
            self.expects_ack,
            "TxReporter: cannot mark as acknowledged a message that does not expect an ACK"
        );
        self.mark(TxState::Transmitted, TxState::Acknowledged);
    }

    /// Transmitted → Lost: LLC did not receive an ACK within the expected time window.
    pub fn mark_lost(&self) {
        assert!(
            self.expects_ack,
            "TxReporter: cannot mark as lost a message that does not expect an ACK"
        );
        self.mark(TxState::Transmitted, TxState::Lost);
    }

    /// Tricky function to re-use linked TxReporters. Resets state to the initial state.
    /// Be very careful when using this.
    pub fn reset(&self) {
        self.mark_unchecked(TxState::Pending);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_observes_signal_transitions() {
        let receipt = TxReporter::new();
        let reporter = receipt.clone();
        assert_eq!(reporter.get_state(), TxState::Pending);
        reporter.mark_transmitted();
        assert_eq!(receipt.get_state(), TxState::Transmitted);
        reporter.mark_acknowledged();
        assert_eq!(receipt.get_state(), TxState::Acknowledged);
    }

    #[test]
    fn cloned_signal_shares_state() {
        let receipt = TxReporter::new();
        let reporter = receipt.clone();
        let reporter2 = reporter.clone();
        reporter2.mark_transmitted();
        assert_eq!(receipt.get_state(), TxState::Transmitted);
        assert_eq!(reporter.get_state(), TxState::Transmitted);
    }

    #[test]
    #[should_panic(expected = "invalid transition")]
    fn double_mark_transmitted_panics() {
        let receipt = TxReporter::new();
        let reporter = receipt.clone();
        reporter.mark_transmitted();
        reporter.mark_transmitted();
    }

    #[test]
    #[should_panic(expected = "cannot mark as acknowledged")]
    fn unacked_mark_acked_panics() {
        let receipt = TxReporter::new_unacked();
        let reporter = receipt.clone();
        reporter.mark_transmitted();
        reporter.mark_acknowledged();
    }

    #[test]
    #[should_panic(expected = "invalid transition")]
    fn mark_acknowledged_from_pending_panics() {
        let receipt = TxReporter::new();
        let reporter = receipt.clone();
        reporter.mark_acknowledged(); // must be Transmitted first
    }
}
