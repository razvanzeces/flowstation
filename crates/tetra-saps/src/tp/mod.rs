use tetra_core::{BitBuffer, BurstType, PhyBlockNum, PhyBlockType, TrainingSequence};

#[derive(Debug, Clone)]
pub struct TpUnitdataInd {
    pub train_type: TrainingSequence,
    pub burst_type: BurstType,
    pub block_type: PhyBlockType,
    /// Undefined for BBK. For all others: [ Block1 | Block2 | Both ]
    pub block_num: PhyBlockNum,
    pub block: BitBuffer,
    /// Received signal strength in dBFS. See RxBurstBits.rssi_dbfs.
    pub rssi_dbfs: f32,
}

#[derive(Debug, Clone)]
pub struct TpUnitdataReqSlot {
    pub train_type: TrainingSequence,
    pub burst_type: BurstType,
    pub bbk: Option<BitBuffer>,
    pub blk1: Option<BitBuffer>,
    pub blk2: Option<BitBuffer>,
}
