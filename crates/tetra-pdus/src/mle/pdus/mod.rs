pub mod d_mle_sync;
pub mod d_mle_sysinfo;

pub mod d_channel_response;
pub mod d_new_cell;
pub mod d_nwrk_broadcast;
pub mod d_nwrk_broadcast_remove;
pub mod d_prepare_fail;
pub mod d_restore_ack;
pub mod d_restore_fail;
mod sdu;
pub mod u_channel_class_advice;
pub mod u_prepare;
pub mod u_restore;

#[cfg(test)]
mod handover_tests {
    use tetra_core::BitBuffer;

    use super::{
        d_new_cell::DNewCell, d_prepare_fail::DPrepareFail, d_restore_ack::DRestoreAck, d_restore_fail::DRestoreFail, u_prepare::UPrepare,
        u_restore::URestore,
    };

    fn bits(s: &str) -> BitBuffer {
        BitBuffer::from_bitstr(s)
    }

    fn encode(f: impl FnOnce(&mut BitBuffer)) -> BitBuffer {
        let mut b = BitBuffer::new_autoexpand(64);
        f(&mut b);
        let len = b.get_pos();
        let mut out = BitBuffer::new(len);
        b.seek(0);
        out.copy_bits(&mut b, len);
        out.seek(0);
        out
    }

    // Layouts follow TS 100 392-2 Annex E.2: the O-bit announces only type 2 elements, the SDU
    // has no P-bit and follows whatever the O-bit says (E.2.1 note 2). Protocol discriminator is
    // not part of these codecs.

    #[test]
    fn u_prepare_layouts_follow_annex_e() {
        // E.27: cell identifier and SDU.
        let p = UPrepare {
            cell_identifier_ca: Some(3),
            sdu: Some(bits("1011001")),
        };
        let mut b = encode(|b| p.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "000".to_string() + "1" + "1" + "00011" + "1011001");
        let q = UPrepare::from_bitbuf(&mut b).unwrap();
        assert_eq!(q.cell_identifier_ca, Some(3));
        assert_eq!(q.sdu.unwrap().to_bitstr(), "1011001");

        // E.26: cell identifier, no SDU.
        let mut b = encode(|b| UPrepare { cell_identifier_ca: Some(7), sdu: None }.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "000".to_string() + "1" + "1" + "00111");
        let q = UPrepare::from_bitbuf(&mut b).unwrap();
        assert_eq!((q.cell_identifier_ca, q.sdu.is_none()), (Some(7), true));

        // SDU without cell identifier: O-bit 0, SDU straight after it.
        let mut b = encode(|b| UPrepare { cell_identifier_ca: None, sdu: Some(bits("1011001")) }.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "000".to_string() + "0" + "1011001");
        let q = UPrepare::from_bitbuf(&mut b).unwrap();
        assert_eq!((q.cell_identifier_ca, q.sdu.unwrap().to_bitstr()), (None, "1011001".to_string()));

        // E.25: nothing optional.
        let mut b = encode(|b| UPrepare { cell_identifier_ca: None, sdu: None }.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "0000");
        let q = UPrepare::from_bitbuf(&mut b).unwrap();
        assert_eq!((q.cell_identifier_ca, q.sdu.is_none()), (None, true));
    }

    #[test]
    fn u_restore_with_la_follows_e28() {
        let r = URestore {
            mcc: None,
            mnc: None,
            la: Some(2),
            sdu: Some(bits("0111000110")),
        };
        let mut b = encode(|b| r.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "100".to_string() + "1" + "0" + "0" + "1" + "00000000000010" + "0111000110");
        let q = URestore::from_bitbuf(&mut b).unwrap();
        assert_eq!((q.mcc, q.mnc, q.la), (None, None, Some(2)));
        assert_eq!(q.sdu.unwrap().to_bitstr(), "0111000110");
    }

    #[test]
    fn u_restore_in_the_same_la_still_carries_the_sdu() {
        // E.29: no MCC/MNC/LA (the MS stayed in the same LA), O-bit 0, U-CALL RESTORE follows.
        let mut b = bits(&("100".to_string() + "0" + "0111000110"));
        let q = URestore::from_bitbuf(&mut b).unwrap();
        assert_eq!((q.mcc, q.mnc, q.la), (None, None, None));
        assert_eq!(q.sdu.unwrap().to_bitstr(), "0111000110");

        let r = URestore {
            mcc: None,
            mnc: None,
            la: None,
            sdu: Some(bits("0111000110")),
        };
        let b = encode(|b| r.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "100".to_string() + "0" + "0111000110");
    }

    #[test]
    fn downlink_pdus_follow_annex_e() {
        // E.19: D-NEW-CELL without SDU.
        let mut b = encode(|b| DNewCell { channel_command_valid: 1, sdu: None }.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "000010");
        assert_eq!(DNewCell::from_bitbuf(&mut b).unwrap().channel_command_valid, 1);

        // E.20: D-NEW-CELL with SDU, O-bit 0.
        let mut b = encode(|b| DNewCell { channel_command_valid: 1, sdu: Some(bits("0101")) }.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "000".to_string() + "01" + "0" + "0101");
        assert_eq!(DNewCell::from_bitbuf(&mut b).unwrap().sdu.unwrap().to_bitstr(), "0101");

        // E.22: D-PREPARE-FAIL with SDU, O-bit 0.
        let mut b = encode(|b| DPrepareFail { fail_cause: 2, sdu: Some(bits("0101")) }.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "001".to_string() + "10" + "0" + "0101");
        let q = DPrepareFail::from_bitbuf(&mut b).unwrap();
        assert_eq!((q.fail_cause, q.sdu.unwrap().to_bitstr()), (2, "0101".to_string()));

        // E.23: D-RESTORE-ACK with D-CALL RESTORE, O-bit 0.
        let mut b = encode(|b| DRestoreAck { sdu: Some(bits("01110")) }.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "100".to_string() + "0" + "01110");
        assert_eq!(DRestoreAck::from_bitbuf(&mut b).unwrap().sdu.unwrap().to_bitstr(), "01110");

        // E.24: D-RESTORE-FAIL.
        let mut b = encode(|b| DRestoreFail { fail_cause: 2 }.to_bitbuf(b).unwrap());
        assert_eq!(b.to_bitstr(), "101100");
        assert_eq!(DRestoreFail::from_bitbuf(&mut b).unwrap().fail_cause, 2);
    }
}
