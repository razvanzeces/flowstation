use crossbeam_channel::Sender;

use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, BurstType, PhyBlockNum, PhyBlockType, Sap, TdmaTime, TrainingSequence, unimplemented_log};
use tetra_pdus::phy::traits::rxtx_dev::RxBurstBits;
use tetra_pdus::phy::traits::rxtx_dev::{RxSlotBits, RxTxDev, TxSlotBits};
use tetra_saps::tp::TpUnitdataInd;
use tetra_saps::{SapMsg, SapMsgInner};

use crate::phy::components::phy_io_file::{FileWriteMsg, PhyIoFileMode};
use crate::phy::components::{burst_consts::*, slotter, train_consts::*};
use crate::umac::subcomp::bs_sched::MACSCHED_TX_AHEAD;
use crate::{MessageQueue, TetraEntityTrait};

use super::components::phy_io_file::PhyIoFile;

pub struct PhyBs<D: RxTxDev> {
    config: SharedConfig,
    dltime: TdmaTime,

    /// Channel for asynchronous downlink TX data logging
    dl_tx_sender: Option<Sender<FileWriteMsg>>,
    /// Channel for asynchronous uplink RX data logging
    ul_rx_sender: Option<Sender<FileWriteMsg>>,

    /// Testing mode: Transmit input data from file instead of from stack
    dl_input_file: Option<PhyIoFile>,
    /// Testing mode: Parse input data from file instead of from SDR
    ul_input_file: Option<PhyIoFile>,

    /// RX/TX device
    rxtxdev: D,

    tick: u64,

    /// Next time a device/file error may be logged at warn level. These fire per
    /// timeslot (~70/s), so an unfiltered warning would bury the journal.
    err_log_next: std::time::Instant,

    /// With a packet-data carrier in use (`[packet_data] pdch_carrier`), or with three or more
    /// carriers (`StackConfig::uplink_copy_rules`), the carriers of a slot are handed over
    /// strongest burst first: the burst a radio really sent comes before its adjacent-channel copy
    /// on another carrier, which the UMAC then drops as the weaker one.
    order_by_rssi: bool,
}

/// The carriers of one received slot, strongest found burst first (non-finite RSSI last, ties in
/// their order).
fn strongest_first(rx: &mut [Option<RxSlotBits<'_>>]) {
    let strongest = |s: &Option<RxSlotBits<'_>>| {
        s.as_ref().map_or(f32::NEG_INFINITY, |s| {
            [&s.slot, &s.subslot1, &s.subslot2]
                .into_iter()
                .filter(|b| b.train_type != TrainingSequence::NotFound && b.rssi_dbfs.is_finite())
                .map(|b| b.rssi_dbfs)
                .fold(f32::NEG_INFINITY, f32::max)
        })
    };
    rx.sort_by(|a, b| strongest(b).total_cmp(&strongest(a)));
}

/// Minimum interval between warnings about a failing timeslot.
const ERR_LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// True if a recurring error may be logged at warn level now. Callers log at debug
/// otherwise, so nothing is lost when tracing is turned up. Takes the deadline by
/// reference rather than &mut self so it can be called while the RX/TX device is borrowed.
fn may_warn(next: &mut std::time::Instant) -> bool {
    let now = std::time::Instant::now();
    if now >= *next {
        *next = now + ERR_LOG_INTERVAL;
        true
    } else {
        false
    }
}

impl<D: RxTxDev> PhyBs<D> {
    pub fn new(config: SharedConfig, rxtxdev: D) -> Self {
        let order_by_rssi = config.config().uplink_copy_rules();
        let c = &config.config().phy_io;

        let dl_tx_logger = c
            .dl_tx_file
            .as_ref()
            .and_then(|f| PhyIoFile::create_async_writer(f, "dl_tx_logger".to_string()).ok());
        let ul_rx_logger = c
            .ul_rx_file
            .as_ref()
            .and_then(|f| PhyIoFile::create_async_writer(f, "ul_rx_logger".to_string()).ok());

        let dl_input_file = if let Some(ref f) = c.dl_input_file {
            Some(PhyIoFile::new(f, PhyIoFileMode::ReadRepeat).expect("Failed to open dl_input_file"))
        } else {
            None
        };
        let ul_input_file = if let Some(ref f) = c.ul_input_file {
            Some(PhyIoFile::new(f, PhyIoFileMode::Read).expect("Failed to open ul_input_file"))
        } else {
            None
        };

        Self {
            config,
            dltime: TdmaTime::default(),
            dl_tx_sender: dl_tx_logger,
            ul_rx_sender: ul_rx_logger,
            dl_input_file,
            ul_input_file,
            rxtxdev,
            tick: 0,
            err_log_next: std::time::Instant::now(),
            order_by_rssi,
        }
    }

    fn send_rxblock_to_lmac(
        queue: &mut MessageQueue,
        carrier_num: u16,
        train_type: TrainingSequence,
        burst_type: BurstType,
        block_type: PhyBlockType,
        block_num: PhyBlockNum,
        bits: BitBuffer,
        rssi_dbfs: f32,
    ) {
        let sapmsg = SapMsg {
            sap: Sap::TpSap,
            src: TetraEntity::Phy,
            dest: TetraEntity::Lmac,
            msg: SapMsgInner::TpUnitdataInd(TpUnitdataInd {
                carrier_num,
                train_type,
                burst_type,
                block_type,
                block_num,
                block: bits,
                rssi_dbfs,
            }),
        };
        queue.push_back(sapmsg);
    }

    fn split_rxslot_and_send_to_lmac(queue: &mut MessageQueue, carrier_num: u16, burst: &RxBurstBits<'_>) {
        let train_seq = burst.train_type;
        match train_seq {
            TrainingSequence::NormalTrainSeq1 => {
                if burst.bits.len() != NUB_BITS {
                    tracing::warn!("PHY: NUB burst wrong length ({} != {}), dropping", burst.bits.len(), NUB_BITS);
                    return;
                }

                let mut blk = BitBuffer::new(NUB_BLK_BITS * 2);
                blk.copy_bits_from_bitarr(&burst.bits[NUB_BLK1_OFFSET..NUB_BLK1_OFFSET + NUB_BLK_BITS]);
                blk.copy_bits_from_bitarr(&burst.bits[NUB_BLK2_OFFSET..NUB_BLK2_OFFSET + NUB_BLK_BITS]);
                blk.seek(0);

                Self::send_rxblock_to_lmac(
                    queue,
                    carrier_num,
                    train_seq,
                    BurstType::NUB,
                    PhyBlockType::NUB,
                    PhyBlockNum::Both,
                    blk,
                    burst.rssi_dbfs,
                );
            }
            TrainingSequence::NormalTrainSeq2 => {
                if burst.bits.len() != NUB_BITS {
                    tracing::warn!("PHY: NUB burst wrong length ({} != {}), dropping", burst.bits.len(), NUB_BITS);
                    return;
                }

                let blk1 = BitBuffer::from_bitarr(&burst.bits[NUB_BLK1_OFFSET..NUB_BLK1_OFFSET + NUB_BLK_BITS]);
                let blk2 = BitBuffer::from_bitarr(&burst.bits[NUB_BLK2_OFFSET..NUB_BLK2_OFFSET + NUB_BLK_BITS]);

                Self::send_rxblock_to_lmac(
                    queue,
                    carrier_num,
                    train_seq,
                    BurstType::NUB,
                    PhyBlockType::NUB,
                    PhyBlockNum::Block1,
                    blk1,
                    burst.rssi_dbfs,
                );
                Self::send_rxblock_to_lmac(
                    queue,
                    carrier_num,
                    train_seq,
                    BurstType::NUB,
                    PhyBlockType::NUB,
                    PhyBlockNum::Block2,
                    blk2,
                    burst.rssi_dbfs,
                );
            }
            TrainingSequence::ExtendedTrainSeq => {
                if burst.bits.len() != CUB_BITS {
                    tracing::warn!("PHY: CUB burst wrong length ({} != {}), dropping", burst.bits.len(), CUB_BITS);
                    return;
                }

                let mut blk = BitBuffer::new(CUB_BLK_BITS * 2);
                blk.copy_bits_from_bitarr(&burst.bits[CUB_BLK1_OFFSET..CUB_BLK1_OFFSET + CUB_BLK_BITS]);
                blk.copy_bits_from_bitarr(&burst.bits[CUB_BLK2_OFFSET..CUB_BLK2_OFFSET + CUB_BLK_BITS]);
                blk.seek(0);

                Self::send_rxblock_to_lmac(
                    queue,
                    carrier_num,
                    train_seq,
                    BurstType::CUB,
                    PhyBlockType::SSN1,
                    PhyBlockNum::Block1,
                    blk,
                    burst.rssi_dbfs,
                );
            }
            other => {
                tracing::debug!("PHY: training sequence {:?} not handled in split_rxslot, dropping", other);
            }
        }
    }

    fn build_dl_burst(&mut self, prim: tetra_saps::tp::TpUnitdataReqSlot) -> Option<[u8; TIMESLOT_TYPE4_BITS]> {
        let mut dl_burst = [0u8; TIMESLOT_TYPE4_BITS];
        if let Some(dl_input_file) = &mut self.dl_input_file {
            if let Err(e) = dl_input_file.read_block(&mut dl_burst) {
                // Replay is a testing mode; a bad input file must not take the cell down.
                if may_warn(&mut self.err_log_next) {
                    tracing::warn!("PHY: failed to read dl_input_file ({:?}), dropping burst", e);
                } else {
                    tracing::debug!("PHY: failed to read dl_input_file ({:?}), dropping burst", e);
                }
                return None;
            }
            return Some(dl_burst);
        }

        let mut bbk = [0u8; 30];
        prim.bbk.expect("TP slot missing BBK").to_bitarr(&mut bbk);

        Some(match prim.burst_type {
            BurstType::SDB => {
                assert!(prim.train_type == TrainingSequence::SyncTrainSeq);
                assert!(prim.blk1.is_some() && prim.blk2.is_some());

                let mut blk1 = [0u8; 120];
                let mut blk2 = [0u8; 216];
                prim.blk1.expect("SDB missing blk1").to_bitarr(&mut blk1);
                prim.blk2.expect("SDB missing blk2").to_bitarr(&mut blk2);
                slotter::build_sdb(&blk1, &bbk, &blk2)
            }
            BurstType::NDB => {
                let mut blk1 = [0u8; 216];
                let mut blk2 = [0u8; 216];

                match prim.train_type {
                    TrainingSequence::NormalTrainSeq1 => {
                        assert!(prim.blk1.is_some() && prim.blk2.is_none());
                        let mut blk1_src = prim.blk1.expect("NDB missing blk1");
                        blk1_src.to_bitarr(&mut blk1);
                        blk1_src.to_bitarr(&mut blk2);
                    }
                    TrainingSequence::NormalTrainSeq2 => {
                        assert!(prim.blk1.is_some() && prim.blk2.is_some());
                        prim.blk1.expect("NDB missing blk1").to_bitarr(&mut blk1);
                        prim.blk2.expect("NDB missing blk2").to_bitarr(&mut blk2);
                    }
                    other => {
                        tracing::warn!("PHY: unsupported training sequence {:?} for NDB burst, dropping", other);
                        return None;
                    }
                }

                slotter::build_ndb(prim.train_type, &blk1, &bbk, &blk2)
            }
            other => {
                tracing::warn!("PHY: unsupported burst type {:?}, dropping", other);
                return None;
            }
        })
    }

    fn rx_tpsap_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        self.tick += 1;

        let prims = match message.msg {
            SapMsgInner::TpUnitdataReq(prim) => vec![prim],
            SapMsgInner::TpUnitdataReqSlots(batch) => batch.slots,
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        };

        let mut dl_bursts = Vec::with_capacity(prims.len());
        let mut carrier_nums = Vec::with_capacity(prims.len());
        for prim in prims {
            let carrier_num = prim.carrier_num;
            if let Some(burst) = self.build_dl_burst(prim) {
                carrier_nums.push(carrier_num);
                dl_bursts.push(burst);
            }
        }

        let tx_time = self.dltime.add_timeslots(MACSCHED_TX_AHEAD as i32);
        let tx_slots = dl_bursts
            .iter()
            .zip(carrier_nums.iter())
            .map(|(burst, carrier_num)| TxSlotBits {
                carrier_num: *carrier_num,
                time: tx_time,
                slot: Some(burst),
                ..Default::default()
            })
            .collect::<Vec<_>>();

        if let Some(dl_tx_sender) = &self.dl_tx_sender {
            for burst in &dl_bursts {
                let _ = dl_tx_sender.try_send(FileWriteMsg::WriteBlock(burst.to_vec()));
            }
        }

        // A failed timeslot must never unwind: everything runs on one thread with no
        // catch_unwind, so a panic here kills the whole cell and systemd crash-loops it.
        // An RX overflow on a loaded RPi + LimeSDR is an everyday event -- drop this
        // timeslot's RX and let the next tick resync. TX has already been queued above.
        let order_by_rssi = self.order_by_rssi;
        let mut rx = match self.rxtxdev.rxtx_timeslot(&tx_slots) {
            Ok(rx) => rx,
            Err(e) => {
                if may_warn(&mut self.err_log_next) {
                    tracing::warn!(ts=%self.dltime, "rxtx_timeslot failed ({:?}), skipping this timeslot", e);
                } else {
                    tracing::debug!(ts=%self.dltime, "rxtx_timeslot failed ({:?}), skipping this timeslot", e);
                }
                return;
            }
        };
        if order_by_rssi {
            strongest_first(&mut rx);
        }

        for rx_slot in rx {
            if let Some(rx_slot) = rx_slot {
                let mut slot_sent = false;
                if rx_slot.slot.train_type != TrainingSequence::NotFound {
                    tracing::info!(ts=%self.dltime, carrier=rx_slot.carrier_num, "rx_tpsap_prim got {:?} in fullslot", rx_slot.slot.train_type);

                    if let Some(ul_rx_sender) = &self.ul_rx_sender {
                        let _ = ul_rx_sender.try_send(FileWriteMsg::WriteHeaderAndBlock(3, self.tick, rx_slot.slot.bits.to_vec()));
                    }

                    Self::split_rxslot_and_send_to_lmac(queue, rx_slot.carrier_num, &rx_slot.slot);
                    slot_sent = true;
                }
                if rx_slot.subslot1.train_type != TrainingSequence::NotFound {
                    tracing::info!(ts=%self.dltime, carrier=rx_slot.carrier_num, "rx_tpsap_prim got {:?} in subslot1", rx_slot.subslot1.train_type);
                    if slot_sent {
                        tracing::debug!("Multiple candidate bursts detected in one slot; forwarding all (LMAC CRC drops false positives)");
                    }
                    if let Some(ul_rx_sender) = &self.ul_rx_sender {
                        let _ = ul_rx_sender.try_send(FileWriteMsg::WriteHeaderAndBlock(1, self.tick, rx_slot.subslot1.bits.to_vec()));
                    }

                    Self::split_rxslot_and_send_to_lmac(queue, rx_slot.carrier_num, &rx_slot.subslot1);
                    slot_sent = true;
                }
                if rx_slot.subslot2.train_type != TrainingSequence::NotFound {
                    tracing::info!(ts=%self.dltime, carrier=rx_slot.carrier_num, "rx_tpsap_prim got {:?} in subslot2", rx_slot.subslot2.train_type);
                    if slot_sent {
                        tracing::debug!("Multiple candidate bursts detected in one slot; forwarding all (LMAC CRC drops false positives)");
                    }
                    if let Some(ul_rx_sender) = &self.ul_rx_sender {
                        let _ = ul_rx_sender.try_send(FileWriteMsg::WriteHeaderAndBlock(2, self.tick, rx_slot.subslot2.bits.to_vec()));
                    }

                    Self::split_rxslot_and_send_to_lmac(queue, rx_slot.carrier_num, &rx_slot.subslot2);
                }
            }
        }
    }

    fn rx_tpc_prim(&mut self, _queue: &mut MessageQueue, _message: SapMsg) {
        unimplemented_log!("rx_tpc_prim: TPC SAP not implemented");
    }
}

impl<D: RxTxDev + Send + 'static> TetraEntityTrait for PhyBs<D> {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Phy
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);

        match message.sap {
            Sap::TpSap => {
                self.rx_tpsap_prim(queue, message);
            }
            Sap::TpcSap => {
                self.rx_tpc_prim(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
            }
        }
    }

    fn tick_start(&mut self, _queue: &mut MessageQueue, ts: TdmaTime) {
        self.dltime = ts;
        let _ = &self.config;
        let _ = &self.ul_input_file;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(carrier_num: u16, rssi_dbfs: f32) -> Option<RxSlotBits<'static>> {
        Some(RxSlotBits {
            carrier_num,
            slot: RxBurstBits {
                train_type: TrainingSequence::NormalTrainSeq1,
                bits: &[],
                rssi_dbfs,
            },
            ..Default::default()
        })
    }

    fn carriers(rx: &[Option<RxSlotBits<'_>>]) -> Vec<Option<u16>> {
        rx.iter().map(|s| s.as_ref().map(|s| s.carrier_num)).collect()
    }

    /// With a packet-data carrier the stronger carrier's burst is handed over first (the real one
    /// before its adjacent-channel copy); non-finite RSSI last, ties in the PHY's order.
    #[test]
    fn carriers_are_delivered_strongest_first_with_a_pdch_carrier() {
        let mut rx = vec![found(1600, -75.0), found(1598, -40.0)];
        strongest_first(&mut rx);
        assert_eq!(carriers(&rx), vec![Some(1598), Some(1600)]);
        let mut rx = vec![found(1600, f32::NEG_INFINITY), found(1598, -90.0), None];
        strongest_first(&mut rx);
        assert_eq!(carriers(&rx), vec![Some(1598), Some(1600), None]);
        let mut rx = vec![found(1600, -60.0), found(1598, -60.0)];
        strongest_first(&mut rx);
        assert_eq!(carriers(&rx), vec![Some(1600), Some(1598)], "a tie keeps the main carrier first");
    }

    #[test]
    fn three_carriers_are_delivered_strongest_first() {
        let mut rx = vec![found(1600, -75.0), found(1598, -60.0), found(1603, -40.0)];
        strongest_first(&mut rx);
        assert_eq!(carriers(&rx), vec![Some(1603), Some(1598), Some(1600)]);
        let mut rx = vec![None, found(1598, -90.0), found(1603, f32::NEG_INFINITY)];
        strongest_first(&mut rx);
        assert_eq!(carriers(&rx), vec![Some(1598), None, Some(1603)], "the rest in the PHY's order");
    }
}
