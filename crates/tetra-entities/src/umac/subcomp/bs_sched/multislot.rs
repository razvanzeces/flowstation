// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.
// SPDX-FileComment: Adapted for flowstation-miura (multislot PDCH on the main carrier and on a packet-data carrier, half-duplex guard) by EA5GVK.

//! Packet-data channels of several slots (`[packet_data] pdch_max_slots` > 1) on the main
//! carrier: what the scheduler does for them besides sharing one downlink queue.
//!
//! The radios that use them (the MXP600) are frequency half duplex without fast switching: they
//! cannot receive while they transmit and need up to one slot to switch (EN 300 392-2 23.1.3.1.1,
//! 23.1.3.1.2). At the BS the uplink frame starts two slots after the downlink one (9.3.9) and the
//! uplink slot labelled u is on air while downlink slot u+2 is, so a radio holding uplink u does
//! not receive downlink u+1 (switching), u+2 (transmitting) and u+3 (switching back). On a channel
//! of two or more slots the BS should not send it anything then (23.1.3.1.1 last paragraph,
//! 23.3.1.2.2 NOTE 2, 23.5.2.2.7):
//!
//! - G1: a downlink slot of the channel its radio does not hear (an uplink slot of its channel
//!   reserved to it at d-1, d-2 or d-3, or the slot right after the one that carried its channel
//!   assignment, 23.5.4.3.1) carries nothing of the channel queue;
//! - G2: while it holds uplink slots of its channel ahead, no downlink fragmentation starts (the
//!   MS drops a partial TM-SDU after N.203 slots without a fragment, 23.4.3.1.1 iii);
//! - a random access of the radio on its channel, which the BS learns of only after the downlink
//!   slots around it went out, drops the fragmented downlink message under way (23.4.2.1.1) and
//!   frees the uplink slots granted in those slots.
//!
//! Uplink capacity the radio asks for on its channel is not granted when the request arrives but
//! when a downlink slot of the channel it hears is built (as Nexus-BS grants pending capacity at
//! transmission): the MS counts the granting delay from the slot that carries the grant, over the
//! slots of its channel (23.5.2.2.2), and which slot that is depends on G1, on frame 18 and on the
//! queue. One debt per channel, the radio's whole requirement less what it already holds ahead
//! (23.5.2.1 NOTE 1), granted in chunks of up to four slots; frame 18 is counted but never
//! granted; no grant while a downlink fragmentation runs on the channel (G3, the grant would
//! break it), and at a message boundary the debt goes first. Once the radio says it has nothing
//! more to send, the slots it still holds are freed (23.5.2.3.1).
//!
//! An advanced link segment that asks for an acknowledgement is granted, with it, a full slot
//! for the answer (23.5.1.3.3, 23.5.2.2.1 b): an AL-ACK sent by random access would be known only
//! once the downlink slots around it went out, a reserved one is covered by G1.
//!
//! With one slot per radio none of this applies.
//!
//! On the packet-data carrier (`[packet_data] pdch_carrier`, a carrier without MCCH) a channel
//! may also hold ts1, and has four slots at most:
//!
//! - its uplink never uses ts1 (the adjacent-channel copy of a burst there would land on the MCCH
//!   uplink of the cell): ts1 is an opportunity the MS counts but never a granted slot, and its
//!   AACH shows the uplink reserved;
//! - the radio arrives from the main carrier's MCCH: it is sent nothing before the first downlink
//!   slot of its channel after the assignment, and taken as transmitting (its linearisation
//!   burst) in the first uplink slot (23.5.4.3.1 rule 1 a, 23.5.4.3.4);
//! - on four slots frame 18 leaves four channel slots without a fragment, which may already be
//!   N.203 (">= 4", annex B.2): no downlink fragmentation starts in frames 14 to 17 (G4), and the
//!   reply slot of an acknowledgement request is four opportunities away.

use super::*;

/// Opportunities before a reply slot at the earliest: the MS decodes the segment and builds its
/// AL-ACK meanwhile (on {3,4}, a segment in (f,4) is answered in (f+1,4) at the earliest).
const PDCH_REPLY_MIN_OPPORTUNITIES: usize = 2;

/// The same on a channel of four slots, where two opportunities are only two timeslots.
const PDCH_REPLY_MIN_OPPORTUNITIES_FOUR: usize = 4;

/// Frames in which no downlink fragmentation starts on a channel of four slots (G4): it would
/// still run across frame 18, four channel slots without a fragment.
const PDCH_FOUR_SLOT_NO_FRAG_FRAMES: std::ops::RangeInclusive<u8> = 14..=17;

/// A radio arriving from another carrier's MCCH is waited for this long at most (a multiframe).
const PDCH_ARRIVAL_WINDOW: i32 = 72;

/// Uplink capacity owed to the radio of a packet-data channel of several slots.
#[derive(Debug, Clone, Copy)]
pub(super) struct ChannelDebt {
    addr: TetraAddress,
    slots: usize,
    /// No grant in this downlink slot or before it.
    not_before: TdmaTime,
    /// The granted slots start at this delay at the earliest.
    min_pos: usize,
}

/// A grant placed in the slot being built: the queue of its channel, its radio, the grant, the
/// uplink slots it reserved and the debt before it, to undo it if it did not leave.
#[derive(Debug)]
pub(super) struct LazyGrant {
    q: usize,
    ssi: u32,
    grant: (BasicSlotgrantCapAlloc, BasicSlotgrantGrantingDelay),
    marker: Option<u8>,
    labels: Vec<TdmaTime>,
    debt: ChannelDebt,
}

impl BsChannelScheduler {
    /// Whether the radio of the packet-data channel `ts` belongs to hears downlink slot `ts`
    /// (G1). Always true for a channel of one slot and for any other slot.
    pub(super) fn pdch_radio_hears(&self, ts: TdmaTime) -> bool {
        // On a carrier without MCCH a channel of one slot has its arrival rule too.
        let owner = self
            .multislot_owner(ts.t)
            .or_else(|| self.pdch_owner(ts.t).filter(|_| !self.allow_mcch()));
        let Some(ssi) = owner else {
            return true;
        };
        let deaf = self.pdch_ms_deaf(ts, ssi, self.pdch_timeslots_of(ssi), self.queue_index(ts.t));
        if deaf {
            tracing::trace!("UMAC: ISSI {} does not hear ts {} of its PDCH at {}", ssi, ts.t, ts);
        }
        !deaf
    }

    /// Whether the radio `ssi` of the channel `channel` (whose queue is `q`) cannot receive
    /// downlink slot `d`: it is transmitting or switching (EN 300 392-2 9.3.9, 23.1.3.1.1,
    /// 23.3.1.2.2 NOTE 2), or `d` comes too soon after its channel assignment (23.5.4.3.1). A
    /// reservation on another slot, the MCCH's say, does not count: once on the channel the MS
    /// does not use grants of the channel it left. A radio sent here from the MCCH of another
    /// carrier receives nothing before the first downlink slot of its channel, and not while it
    /// sends its linearisation burst in the first uplink slot (23.5.4.3.4).
    fn pdch_ms_deaf(&self, d: TdmaTime, ssi: u32, channel: [bool; 4], q: usize) -> bool {
        self.pdch_arrival[q].is_some_and(|(d1, u1)| (1..PDCH_ARRIVAL_WINDOW).contains(&d1.diff(d)) || (1..=3).contains(&d.diff(u1)))
            || self.pdch_assigned_at[q].is_some_and(|a| a.add_timeslots(1) == d)
            || (1..=3).any(|k| {
                let u = d.add_timeslots(-k);
                channel[u.t as usize - 1] && self.ul_reserved_to(u, ssi)
            })
    }

    /// Whether uplink slot `u` is reserved to `ssi`.
    pub fn ul_reserved_to(&self, u: TdmaTime, ssi: u32) -> bool {
        let e = &self.ulsched[u.t as usize - 1][self.ul_ts_to_sched_index(&u)];
        e.ul1 == Some(ssi) || e.ul2 == Some(ssi)
    }

    /// Whether uplink slot `u` is reserved to `ssi` for a burst on `lchan`: a slot is sent as
    /// SCH/F, a subslot as SCH/HU (EN 300 392-2 23.5.2.3.1), never as STCH. Both subslots of one
    /// slot granted to the radio read as the slot.
    pub fn ul_reserved_as(&self, u: TdmaTime, ssi: u32, lchan: LogicalChannel) -> bool {
        let e = &self.ulsched[u.t as usize - 1][self.ul_ts_to_sched_index(&u)];
        let (one, two) = (e.ul1 == Some(ssi), e.ul2 == Some(ssi));
        match lchan {
            LogicalChannel::SchF => one && two,
            LogicalChannel::SchHu => one != two,
            _ => false,
        }
    }

    /// Free uplink slot `u`.
    fn ul_free_slot(&mut self, u: TdmaTime) {
        let index = self.ul_ts_to_sched_index(&u);
        let e = &mut self.ulsched[u.t as usize - 1][index];
        e.ul1 = None;
        e.ul2 = None;
        e.usage_marker = None;
    }

    /// Whether `ssi` holds uplink slots of its channel `channel` from label `d-3` on (G2). The
    /// scan stops at `d+67`: `d+68` is the schedule entry of `d-4`, cleared only once `d` is built.
    fn pdch_ul_ahead(&self, d: TdmaTime, ssi: u32, channel: [bool; 4]) -> bool {
        (-3..=67).any(|k| {
            let u = d.add_timeslots(k);
            channel[u.t as usize - 1] && self.ul_reserved_to(u, ssi)
        })
    }

    /// Whether the block of downlink slot `ts` may only take PDUs that go whole in it (G2): its
    /// radio holds uplink slots of its channel ahead, or is owed some (between two chunks of an
    /// uplink fragmentation there are none ahead, and a downlink fragmentation started then would
    /// hold the next chunk, G3, beyond T.202, 23.4.2.1.2 b). Evaluated after the grant placed in
    /// this slot.
    pub(super) fn pdch_whole_only(&self, ts: TdmaTime) -> bool {
        self.multislot_owner(ts.t).is_some_and(|ssi| {
            let channel = self.pdch_timeslots_of(ssi);
            self.pdch_channel_debt[self.queue_index(ts.t)].is_some()
                || self.pdch_ul_ahead(ts, ssi, channel)
                || (Self::width(channel) == 4 && PDCH_FOUR_SLOT_NO_FRAG_FRAMES.contains(&ts.f))
        })
    }

    /// Number of slots of a channel.
    fn width(channel: [bool; 4]) -> usize {
        channel.iter().filter(|set| **set).count()
    }

    /// The last of the PDU `fragger` sends went out in downlink slot `ts`: if it assigns a
    /// packet-data channel of several slots of this carrier, the first slot of the new channel
    /// the MS receives is the first one starting at least one slot after the end of this one
    /// (23.5.4.3.1 rule 1, NOTE 7 ii). Not where it was first taken: with no room left there it
    /// waits for the next block, fragmented it moves the MS only once all out (23.4.2.1.1).
    ///
    /// On the main carrier with a packet-data carrier in use, an individual assignment to that
    /// carrier is listed for the UMAC, which hands the radio over to the carrier's scheduler.
    pub(super) fn pdch_note_assignment(&mut self, ts: TdmaTime, fragger: &BsFragger) {
        let Some(c) = fragger.chan_alloc() else {
            return;
        };
        if c.carrier_num == self.carrier_num
            && c.ts_assigned.iter().filter(|set| **set).count() > 1
            && let Some(lowest) = c.ts_assigned.iter().position(|set| *set)
        {
            self.pdch_assigned_at[lowest] = Some(ts);
        } else if self.pdch_handoff_carrier == Some(c.carrier_num)
            && c.ts_assigned.iter().any(|set| *set)
            && !fragger.is_for_group()
            && let Some(ssi) = fragger.ssi()
        {
            self.pdch_assignments_elsewhere.push((ssi, c.ts_assigned, ts));
        }
    }

    /// The radio `ssi` was assigned its packet-data channel `channel` of this carrier in downlink
    /// slot `a` of another carrier's MCCH: the first downlink slot of the channel it receives is
    /// the first one starting at least a slot after the end of `a`, and its first uplink slot the
    /// first one of the channel starting then (EN 300 392-2 23.5.4.3.1 rule 1 a and NOTE 7; the
    /// uplink slot labelled u is on air with downlink u+2, 9.3.9). It may send its linearisation
    /// burst in that uplink slot (23.5.4.3.4).
    pub fn pdch_note_assignment_from_mcch(&mut self, ssi: u32, channel: [bool; 4], a: TdmaTime) {
        let Some(lowest) = channel.iter().position(|set| *set) else {
            return;
        };
        let q = self.queue_index(lowest as u8 + 1);
        let first = |from: TdmaTime| (0..8).map(|k| from.add_timeslots(k)).find(|t| channel[t.t as usize - 1]);
        let (Some(d1), Some(u1)) = (first(a.add_timeslots(2)), first(a)) else {
            return;
        };
        self.pdch_arrival[q] = Some((d1, u1));
        tracing::debug!(
            "UMAC: ISSI {} assigned its PDCH of carrier {} in {}: first downlink slot {}, first uplink slot {}",
            ssi,
            self.carrier_num,
            a,
            d1,
            u1
        );
    }

    /// A PDU of `ssi` came in uplink slot `label` (PHY block `block`) of its channel of several
    /// slots. In a slot not reserved to it, it is a random access, which the BS learns of only
    /// once the downlink slots around it went out: the MS lost what was sent in them (23.5.1.4.12
    /// lets it skip such an access, not undo it), so the fragmented downlink message under way on
    /// the channel is dropped ("by sending no more fragments", 23.4.2.1.1) rather than sent to
    /// its end for nothing. Dropping it reports its TM-SDU discarded. The uplink slots granted in
    /// downlink `label`+1..+3 are freed: the MS will not use them, and a requirement it sends
    /// would be taken as covered by them (23.5.2.1 NOTE 1 counts what it holds); the PDU's own
    /// requirement, or its lack of one, then sets what it is owed.
    pub fn pdch_random_access(&mut self, label: TdmaTime, block: PhyBlockNum, ssi: u32) {
        if self.multislot_owner(label.t) != Some(ssi) || self.ul_get_slot_owner(label, block) == Some(ssi) {
            return;
        }
        let q = self.queue_index(label.t);
        let before = self.dltx_queues[q].len();
        self.dltx_queues[q].retain(|e| !matches!(e, DlSchedElem::FragBuf(f) if f.is_started()));
        if self.dltx_queues[q].len() < before {
            tracing::info!(
                "UMAC: ISSI {} random access on ts {} of its PDCH: the downlink message being fragmented to it dropped",
                ssi,
                label.t
            );
        }
        let unheard: Vec<TdmaTime> = self
            .pdch_grants_out
            .iter()
            .filter(|(d, s, _)| *s == ssi && (1..=3).contains(&d.diff(label)))
            .flat_map(|(_, _, labels)| labels.iter().copied())
            .filter(|u| self.ul_reserved_to(*u, ssi))
            .collect();
        for u in &unheard {
            self.ul_free_slot(*u);
        }
        if !unheard.is_empty() {
            tracing::info!(
                "UMAC: ISSI {} random access on ts {} of its PDCH: {} uplink slot(s) granted while it transmitted freed",
                ssi,
                label.t,
                unheard.len()
            );
        }
    }

    /// A slot of the channel of several slots of `ssi` stops being its. Granted capacity cannot be
    /// withdrawn (23.5.2.2.2) and the MS sends in granted slots without reading the AACH
    /// (23.5.2.2.4 NOTE 2): it may still transmit in the slots it holds there, into whatever took
    /// them. Logged for every slot of the channel when its first slot goes.
    pub(super) fn pdch_note_uplink_left_behind(&self, ssi: u32) {
        let channel = self.pdch_timeslots_of(ssi);
        if channel.iter().filter(|set| **set).count() < 2 {
            return;
        }
        for ts in (1..=4u8).filter(|ts| channel[usize::from(*ts) - 1]) {
            let held: Vec<TdmaTime> = (0..=67)
                .map(|k| self.cur_dltime.add_timeslots(k))
                .filter(|u| u.t == ts && self.ul_reserved_to(*u, ssi))
                .collect();
            if let Some(last) = held.last() {
                tracing::info!(
                    "UMAC: ts {} leaves the PDCH of ISSI {} while it still holds {} reserved uplink slot(s) on it, the last at {}",
                    ts,
                    ssi,
                    held.len(),
                    last
                );
            }
        }
    }

    /// Uplink slots of `channel` after `label`, up to the end of the live schedule, reserved to
    /// `ssi`.
    fn ul_held_after(&self, label: TdmaTime, ssi: u32, channel: [bool; 4]) -> Vec<TdmaTime> {
        let span = self.cur_dltime.add_timeslots(67).diff(label);
        (1..=span)
            .map(|k| label.add_timeslots(k))
            .filter(|u| channel[u.t as usize - 1] && self.ul_reserved_to(*u, ssi))
            .collect()
    }

    /// Uplink opportunities of the channel `channel` counted from uplink slot `from`, the
    /// same-numbered slot of the downlink slot carrying the grant: the delay and the `n`
    /// successive channel slots of the first free run starting at position `min_pos` or later
    /// (EN 300 392-2 23.5.2.2.2). Every slot of the channel counts, frame 18 included; a run never
    /// starts in frame 18, jumps a common linearization slot there and ends at any other frame-18
    /// slot (the MS would use it, and frame 18 is never granted). On a carrier without MCCH uplink
    /// ts1 of the channel counts too but is never granted: a run ends there. None when no run
    /// starts within the largest delay.
    fn ul_find_channel_opportunity(&self, from: TdmaTime, channel: [bool; 4], n: usize, min_pos: usize) -> Option<(usize, Vec<TdmaTime>)> {
        let (mut pos, mut start, mut run) = (0usize, 0usize, Vec::with_capacity(n));
        for dist in 0..(MACSCHED_NUM_FRAMES - 1) * NUM_TIMESLOTS {
            let c = from.add_timeslots(dist as i32);
            if !channel[c.t as usize - 1] {
                continue;
            }
            let i = pos;
            pos += 1;
            if run.is_empty() && i > MAX_GRANTING_DELAY_OPPORTUNITIES {
                return None;
            }
            if c.f == 18 {
                if !run.is_empty() && !c.is_mandatory_clch() {
                    run.clear();
                }
                continue;
            }
            if c.t == 1 && !self.allow_mcch() {
                run.clear();
                continue;
            }
            if run.is_empty() && i < min_pos {
                continue;
            }
            let e = &self.ulsched[c.t as usize - 1][self.ul_ts_to_sched_index(&c)];
            if e.ul1.is_none() && e.ul2.is_none() {
                if run.is_empty() {
                    start = i;
                }
                run.push(c);
            } else {
                run.clear();
            }
            if run.len() == n {
                return Some((start, run));
            }
        }
        None
    }

    /// A capacity request of the radio of a channel of several slots, sent in uplink slot `label`
    /// of it: its requirement, less the slots of the channel it already holds ahead (the MS gives
    /// its whole estimate "irrespective of whether or not [it] has already been granted some
    /// future slots", 23.5.2.1 NOTE 1), becomes the channel's debt, granted when a slot of the
    /// channel it hears is built. False for any other request, which goes the usual way.
    pub fn ul_defer_to_channel(&mut self, label: TdmaTime, addr: TetraAddress, res_req: &ReservationRequirement) -> bool {
        if self.multislot_owner(label.t) != Some(addr.ssi) {
            return false;
        }
        // A subslot: the MS may use a granted full slot for its message (23.5.2.3.1).
        let wanted = match res_req {
            ReservationRequirement::Req1Subslot => 1,
            other => other.to_req_slotcount(),
        };
        let ahead = self.ul_held_after(label, addr.ssi, self.pdch_timeslots_of(addr.ssi)).len();
        let owed = wanted.saturating_sub(ahead);
        let q = self.queue_index(label.t);
        self.pdch_channel_debt[q] = (owed > 0).then_some(ChannelDebt {
            addr,
            slots: owed,
            not_before: self.cur_dltime,
            min_pos: 0,
        });
        tracing::debug!(
            "UMAC: ISSI {} asks for {} uplink slot(s) on its PDCH at {}, holds {} ahead: {} owed",
            addr.ssi,
            wanted,
            label,
            ahead,
            owed
        );
        true
    }

    /// The radio of a channel of several slots sent, in uplink slot `label` of it, a PDU saying it
    /// has nothing more to send (a Null PDU, a MAC-DATA or MAC-END without reservation
    /// requirement, an SCH/HU block without one, a MAC-U-BLCK saying none): it will not use the
    /// capacity it still holds (23.5.2.3.1). Those slots are freed (they would keep the radio deaf
    /// for G1 and the AACH reserved) and its debt dropped.
    pub fn ul_release_after(&mut self, label: TdmaTime, ssi: u32) {
        if self.multislot_owner(label.t) != Some(ssi) {
            return;
        }
        let held = self.ul_held_after(label, ssi, self.pdch_timeslots_of(ssi));
        for u in &held {
            self.ul_free_slot(*u);
        }
        let owed = self.pdch_channel_debt[self.queue_index(label.t)].take();
        if !held.is_empty() || owed.is_some() {
            tracing::debug!(
                "UMAC: ISSI {} has nothing more to send on its PDCH: {} reserved uplink slot(s) freed{}",
                ssi,
                held.len(),
                if owed.is_some() { ", its debt dropped" } else { "" }
            );
        }
    }

    /// The radio `ssi` moved to its packet-data channel of another carrier in downlink slot
    /// `label`: the uplink slots it holds here after it are freed (grants on the channel it left
    /// cease to be valid, 23.5.4.3.1). Returns how many.
    pub fn ul_release_for_handoff(&mut self, label: TdmaTime, ssi: u32) -> usize {
        let held = self.ul_held_after(label, ssi, [true; 4]);
        for u in &held {
            let index = self.ul_ts_to_sched_index(u);
            let e = &mut self.ulsched[u.t as usize - 1][index];
            e.ul1 = e.ul1.filter(|s| *s != ssi);
            e.ul2 = e.ul2.filter(|s| *s != ssi);
            if e.ul1.is_none() && e.ul2.is_none() {
                e.usage_marker = None;
            }
        }
        held.len()
    }

    /// An advanced link segment asking for an acknowledgement is queued to `ssi`, the radio of a
    /// channel of several slots (the LLC tags it `DATA_CATEGORY_AL_REPLY`): its reporter.
    pub fn pdch_want_reply(&mut self, ssi: u32, reporter: TxReporter) {
        if self.pdch.iter().filter(|o| **o == Some(ssi)).count() > 1 {
            self.pdch_reply_wanted.push((ssi, reporter));
        }
    }

    /// Whether the first PDU in the queue `q` for `ssi` is a segment that asks for an answer.
    fn pdch_first_pdu_wants_a_reply(&self, q: usize, ssi: u32) -> bool {
        self.dltx_queues[q]
            .iter()
            .find_map(|e| match e {
                DlSchedElem::Resource(pdu, _, reporter, _) if pdu.addr.is_some_and(|a| a.ssi == ssi && a.ssi_type != SsiType::Gssi) => {
                    Some(reporter.as_ref())
                }
                _ => None,
            })
            .flatten()
            .is_some_and(|r| self.pdch_reply_wanted.iter().any(|(s, w)| *s == ssi && w.same(r)))
    }

    /// Whether a resource with a slot grant and a usage marker added goes whole in one SCH/F.
    fn fits_with_a_grant(pdu: &MacResource, sdu: &BitBuffer) -> bool {
        let mut with = pdu.clone();
        with.slot_granting_element = Some(BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::Grant4Slots,
            granting_delay: BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity,
        });
        with.usage_marker = Some(4);
        with.compute_header_len() + sdu.get_len() <= SCH_F_CAP
    }

    /// Before downlink slot `d` of a channel of several slots is built: grant its radio the next
    /// chunk of what it is owed, counted from `d`, the largest (up to four slots) whose delay
    /// fits. The grant rides on the first PDU for the radio that goes whole in one block with it
    /// (not on a channel allocation: a grant there is one on the channel the MS leaves and delays
    /// the move, 23.5.4.3.1 rule 1 b; not on a group PDU), moved to the front of the queue so it
    /// leaves in `d`; else on a MAC-RESOURCE of its own. Not in frame 18, not before the slots
    /// of the previous chunk, not while a downlink fragmentation runs on the channel (G3).
    pub(super) fn pdch_lazy_grant(&mut self, d: TdmaTime) {
        let Some(ssi) = self.multislot_owner(d.t) else {
            return;
        };
        let q = self.queue_index(d.t);
        // A segment asking for an acknowledgement first in line: a slot for the answer, unless
        // the radio is owed some already (it answers in any granted slot, 23.5.2.3.1).
        self.pdch_reply_wanted
            .retain(|(_, r)| r.get_state() == tetra_core::TxState::Pending);
        if self.pdch_channel_debt[q].is_none() && self.pdch_first_pdu_wants_a_reply(q, ssi) {
            self.pdch_channel_debt[q] = Some(ChannelDebt {
                addr: TetraAddress::issi(ssi),
                slots: 1,
                not_before: d.add_timeslots(-1),
                min_pos: if Self::width(self.pdch_timeslots_of(ssi)) == 4 {
                    PDCH_REPLY_MIN_OPPORTUNITIES_FOUR
                } else {
                    PDCH_REPLY_MIN_OPPORTUNITIES
                },
            });
        }
        let Some(debt) = self.pdch_channel_debt[q] else {
            return;
        };
        if d.f == 18 || d.diff(debt.not_before) <= 0 {
            return;
        }
        let queue = &self.dltx_queues[q];
        // G3, and a PDU deferred for want of room, which would take the block before the grant.
        if queue
            .iter()
            .any(|e| matches!(e, DlSchedElem::FragBuf(f) if f.is_started() || !f.is_packet_data()))
        {
            return;
        }
        // A grant already on its way goes as it is.
        if queue.iter().any(
            |e| matches!(e, DlSchedElem::Resource(pdu, ..) if pdu.slot_granting_element.is_some() && pdu.addr.is_some_and(|a| a.ssi == ssi)),
        ) {
            return;
        }
        let channel = self.pdch_timeslots_of(ssi);
        let Some((n, delay, labels)) = (1..=debt.slots.min(PDCH_UL_GRANT_CHUNK_SLOTS)).rev().find_map(|n| {
            self.ul_find_channel_opportunity(d, channel, n, debt.min_pos)
                .map(|(delay, labels)| (n, delay, labels))
        }) else {
            tracing::debug!("UMAC: no uplink slot of its PDCH within a granting delay for ISSI {} at {}", ssi, d);
            return;
        };
        let marker = (n >= 2).then(|| self.alloc_usage_marker(q as u8 + 1));
        let grant = BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::from_req_slotcount(n),
            granting_delay: if delay == 0 {
                BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity
            } else {
                BasicSlotgrantGrantingDelay::DelayNOpportunities(delay as u8)
            },
        };
        self.ul_reserve_grant(ssi, labels.clone(), false, marker);
        let carrier = self.dltx_queues[q].iter().position(|e| {
            matches!(e, DlSchedElem::Resource(pdu, sdu, ..)
                if pdu.addr.is_some_and(|a| a.ssi == ssi && a.ssi_type != SsiType::Gssi)
                    && pdu.chan_alloc_element.is_none()
                    && Self::fits_with_a_grant(pdu, sdu))
        });
        let elem = match carrier {
            Some(i) => {
                let mut elem = self.dltx_queues[q].remove(i);
                if let DlSchedElem::Resource(pdu, ..) = &mut elem {
                    pdu.slot_granting_element = Some(grant.clone());
                    if pdu.usage_marker.is_none() {
                        pdu.usage_marker = marker;
                    }
                }
                elem
            }
            None => {
                let mut pdu = Self::dl_make_minimal_resource(&debt.addr, Some(grant.clone()), false);
                pdu.usage_marker = marker;
                DlSchedElem::Resource(pdu, BitBuffer::new(0), None, false)
            }
        };
        self.dltx_queues[q].insert(0, elem);
        let last = *labels.last().expect("a grant has slots");
        self.pdch_channel_debt[q] = (debt.slots > n).then_some(ChannelDebt {
            addr: debt.addr,
            slots: debt.slots - n,
            not_before: last,
            min_pos: 0,
        });
        tracing::debug!(
            "UMAC: ISSI {} granted {} of {} uplink slot(s) of its PDCH in {}, delay {}, {} to {}",
            ssi,
            n,
            debt.slots,
            d,
            delay,
            labels[0],
            last
        );
        self.pdch_lazy = Some(LazyGrant {
            q,
            ssi,
            grant: (grant.capacity_allocation, grant.granting_delay),
            marker,
            labels,
            debt,
        });
    }

    /// After downlink slot `d` was built: a grant placed in it that did not leave in it (the
    /// delay was counted from `d`) is taken back, which is possible because the MS never saw it:
    /// its slots are freed and the debt restored.
    pub(super) fn pdch_check_lazy_grant(&mut self, d: TdmaTime) {
        let Some(lazy) = self.pdch_lazy.take() else {
            return;
        };
        // Still queued: as it was, or turned into a fragger none of which went out (no room).
        let mut found = None;
        for (i, e) in self.dltx_queues[lazy.q].iter_mut().enumerate() {
            let (pdu, empty) = match e {
                DlSchedElem::Resource(pdu, sdu, reporter, _) if pdu.addr.is_some_and(|a| a.ssi == lazy.ssi) => {
                    let empty = sdu.get_len() == 0 && reporter.is_none() && !pdu.random_access_flag;
                    (pdu, empty)
                }
                DlSchedElem::FragBuf(f) if f.ssi() == Some(lazy.ssi) => match f.unsent_resource_mut() {
                    Some(pdu) => (pdu, false),
                    None => continue,
                },
                _ => continue,
            };
            if !pdu
                .slot_granting_element
                .as_ref()
                .is_some_and(|g| (g.capacity_allocation, g.granting_delay) == lazy.grant)
            {
                continue;
            }
            pdu.slot_granting_element = None;
            if pdu.usage_marker == lazy.marker {
                pdu.usage_marker = None;
            }
            found = Some((i, empty));
            break;
        }
        let Some((i, empty)) = found else {
            // It left: a random access of its radio in the uplink slots before may show that the
            // radio did not hear it.
            self.pdch_grants_out.retain(|(t, ..)| d.diff(*t) < 4);
            self.pdch_grants_out.push((d, lazy.ssi, lazy.labels));
            return;
        };
        if empty {
            self.dltx_queues[lazy.q].remove(i);
        }
        for u in &lazy.labels {
            if self.ul_reserved_to(*u, lazy.ssi) {
                self.ul_free_slot(*u);
            }
        }
        self.pdch_channel_debt[lazy.q] = Some(lazy.debt);
        tracing::warn!("UMAC: the PDCH grant for ISSI {} did not leave in {}, taken back", lazy.ssi, d);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{carrier_slotter, dl_pdus, finalize_slots, get_testing_slotter, multislot_slotter};
    use super::*;

    const RADIO: u32 = 2_145_007;

    fn radio() -> TetraAddress {
        TetraAddress::issi(RADIO)
    }

    fn at(f: u8, t: u8) -> TdmaTime {
        TdmaTime { t, f, m: 1, h: 0 }
    }

    /// A MAC-RESOURCE for the radio with a TM-SDU of `bits`.
    fn resource(bits: usize) -> (MacResource, BitBuffer) {
        let sdu: String = (0..bits).map(|i| if i % 3 == 0 { '1' } else { '0' }).collect();
        (
            BsChannelScheduler::dl_make_minimal_resource(&radio(), None, false),
            BitBuffer::from_bitstr(&sdu),
        )
    }

    /// The scheduler of a channel `channel` of the radio, its next slot built (4,1).
    fn channel_sched(channel: &[u8]) -> BsChannelScheduler {
        let mut sched = multislot_slotter(RADIO, channel);
        sched.set_dl_time(at(3, 3));
        sched
    }

    /// Slots (frame, ts) of the channel `channel`, frames 4 to 7, that carried nothing for its
    /// radio while PDUs for it were always queued, with its uplink reserved at `reserved`.
    fn quiet_slots(channel: &[u8], reserved: &[(u8, u8)]) -> Vec<(u8, u8)> {
        let mut sched = channel_sched(channel);
        let labels: Vec<TdmaTime> = reserved.iter().map(|(f, t)| at(*f, *t)).collect();
        if !labels.is_empty() {
            sched.ul_reserve_grant(RADIO, labels, false, None);
        }
        for _ in 0..20 {
            let (pdu, sdu) = resource(200);
            sched.dl_enqueue_tma_for_link(u32::from(channel[0]), pdu, sdu, None);
        }
        finalize_slots(&mut sched, 16)
            .iter()
            .filter(|s| channel.contains(&s.ts.t) && !dl_pdus(s).iter().any(|p| p.1 == Some(RADIO)))
            .map(|s| (s.ts.f, s.ts.t))
            .collect()
    }

    /// EN 300 392-2 9.3.9 and 23.1.3.1.1: a radio holding uplink u does not hear downlink u+1,
    /// u+2 and u+3; of those, only the slots of its channel stay empty.
    #[test]
    fn test_half_duplex_guard_blocks_the_slots_around_the_uplink() {
        assert_eq!(quiet_slots(&[3, 4], &[]), vec![]);
        assert_eq!(quiet_slots(&[3, 4], &[(5, 3)]), vec![(5, 4)]);
        assert_eq!(quiet_slots(&[3, 4], &[(5, 4)]), vec![(6, 3)]);
        assert_eq!(quiet_slots(&[2, 3, 4], &[(5, 2)]), vec![(5, 3), (5, 4)]);
        assert_eq!(quiet_slots(&[2, 3, 4], &[(5, 3)]), vec![(5, 4), (6, 2)]);
        assert_eq!(quiet_slots(&[2, 3, 4], &[(5, 4)]), vec![(6, 2), (6, 3)]);
        assert_eq!(quiet_slots(&[4], &[(5, 4)]), vec![], "one slot: no guard");
    }

    /// A reservation of the radio on the MCCH is not one of its channel: once on the channel the
    /// MS does not use grants of the channel it left (23.5.4.3.1).
    #[test]
    fn test_the_guard_ignores_mcch_reservations() {
        assert_eq!(quiet_slots(&[3, 4], &[(5, 1)]), vec![]);
    }

    /// The first channel slot the MS receives after its assignment in (4,1) is the one starting
    /// at least a slot after it (23.5.4.3.1 rule 1): (4,3), not (4,2). With one slot nothing
    /// changes.
    #[test]
    fn test_the_slot_after_the_assignment_is_not_used() {
        use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
        use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
        for (channel, first) in [(&[2u8, 3, 4][..], 3u8), (&[2u8][..], 2u8)] {
            let mut sched = channel_sched(channel);
            let mut ts_assigned = [false; 4];
            for ts in channel {
                ts_assigned[usize::from(*ts) - 1] = true;
            }
            let mut assignment = BsChannelScheduler::dl_make_minimal_resource(&radio(), None, false);
            assignment.chan_alloc_element = Some(ChanAllocElement {
                alloc_type: ChanAllocType::Replace,
                ts_assigned,
                ul_dl_assigned: UlDlAssignment::Both,
                clch_permission: false,
                cell_change_flag: false,
                carrier_num: sched.carrier_num(),
                ext: None,
                mon_pattern: 1,
                frame18_mon_pattern: None,
            });
            sched.dl_enqueue_tma_for_link(0, assignment, BitBuffer::from_bitstr("1011001110001111"), None);
            let (pdu, sdu) = resource(64);
            sched.dl_enqueue_tma_for_link(2, pdu, sdu, None);
            let slots = finalize_slots(&mut sched, 4);
            assert_eq!(slots[0].ts, at(4, 1));
            let carried: Vec<u8> = slots
                .iter()
                .filter(|s| s.ts.t != 1 && dl_pdus(s).iter().any(|p| p.1 == Some(RADIO)))
                .map(|s| s.ts.t)
                .collect();
            assert_eq!(carried.first(), Some(&first), "channel {channel:?}: {carried:?}");
        }
    }

    /// An assignment that does not fit in the rest of the MCCH block it is taken for waits for the
    /// next one: the slot not received is the one after the slot it goes out in, (5,2), not (4,2).
    #[test]
    fn test_the_slot_after_a_deferred_assignment_is_not_used() {
        use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
        use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
        let mut sched = channel_sched(&[2, 3, 4]);
        // 240 bits for another radio first in (4,1): 28 are left, too few for the assignment.
        let other = BsChannelScheduler::dl_make_minimal_resource(&TetraAddress::issi(RADIO + 1), None, false);
        let sdu: String = (0..197).map(|i| if i % 2 == 0 { '1' } else { '0' }).collect();
        sched.dl_enqueue_tma_for_link(0, other, BitBuffer::from_bitstr(&sdu), None);
        let mut assignment = BsChannelScheduler::dl_make_minimal_resource(&radio(), None, false);
        assignment.chan_alloc_element = Some(ChanAllocElement {
            alloc_type: ChanAllocType::Replace,
            ts_assigned: [false, true, true, true],
            ul_dl_assigned: UlDlAssignment::Both,
            clch_permission: false,
            cell_change_flag: false,
            carrier_num: sched.carrier_num(),
            ext: None,
            mon_pattern: 1,
            frame18_mon_pattern: None,
        });
        sched.dl_enqueue_tma_for_link(0, assignment, BitBuffer::from_bitstr("1011001110001111"), None);
        let slots = finalize_slots(&mut sched, 8);
        let sent_in: Vec<TdmaTime> = slots
            .iter()
            .filter(|s| {
                resources(s)
                    .iter()
                    .any(|r| r.addr.is_some_and(|a| a.ssi == RADIO) && r.chan_alloc_element.is_some())
            })
            .map(|s| s.ts)
            .collect();
        assert_eq!(sent_in, vec![at(5, 1)]);
        assert_eq!(sched.pdch_assigned_at[1], Some(at(5, 1)));
        assert!(sched.pdch_radio_hears(at(4, 2)));
        assert!(!sched.pdch_radio_hears(at(5, 2)));
    }

    /// A datagram for the radio waits while the radio holds uplink slots of its channel ahead
    /// (from d-3): a fragmentation started in between would lose the slots it is deaf in.
    #[test]
    fn test_no_downlink_fragmentation_before_a_granted_uplink() {
        for packet_data in [false, true] {
            let mut sched = channel_sched(&[3, 4]);
            sched.ul_reserve_grant(RADIO, vec![at(6, 3)], false, None);
            let (pdu, sdu) = resource(600);
            if packet_data {
                sched.dl_enqueue_packet_data_for_link(3, pdu, sdu, None);
            } else {
                sched.dl_enqueue_tma_for_link(3, pdu, sdu, None);
            }
            let start = finalize_slots(&mut sched, 16)
                .iter()
                .find(|s| dl_pdus(s).iter().any(|p| p.0 == 0 && p.2 == 0b111111))
                .map(|s| (s.ts.f, s.ts.t));
            assert_eq!(start, Some((7, 3)), "packet data {packet_data}");
        }
    }

    /// The scan of G2 stops at d+67: the entry of d-4, still holding its old reservation while d
    /// is built, is not taken for one 68 slots ahead.
    #[test]
    fn test_the_guard_scan_does_not_wrap() {
        let mut sched = multislot_slotter(RADIO, &[3, 4]);
        // The slot built next is (6,1).
        sched.set_dl_time(at(5, 3));
        sched.ul_reserve_grant(RADIO, vec![at(5, 3)], false, None);
        let (pdu, sdu) = resource(600);
        sched.dl_enqueue_tma_for_link(3, pdu, sdu, None);
        let slots = finalize_slots(&mut sched, 3);
        assert_eq!(slots[2].ts, at(6, 3));
        assert_eq!(dl_pdus(&slots[2]).first().map(|p| (p.0, p.2)), Some((0, 0b111111)));
    }

    /// A slot reserved to the radio is one for an SCH/F burst, a subslot one for an SCH/HU burst,
    /// neither for an STCH; both subslots of one slot granted to it read as the slot.
    #[test]
    fn test_ul_reserved_as_matches_the_granted_form() {
        use LogicalChannel::{SchF, SchHu, Stch};
        let mut sched = get_testing_slotter();
        sched.set_dl_time(at(3, 3));
        let (slot, subslot) = (at(5, 3), at(6, 3));
        sched.ul_reserve_grant(RADIO, vec![slot], false, None);
        assert_eq!(
            [SchF, SchHu, Stch].map(|l| sched.ul_reserved_as(slot, RADIO, l)),
            [true, false, false]
        );
        assert!(!sched.ul_reserved_as(slot, RADIO + 1, SchF), "another radio");
        sched.ul_reserve_grant(RADIO, vec![subslot], true, None);
        assert_eq!(
            [SchF, SchHu, Stch].map(|l| sched.ul_reserved_as(subslot, RADIO, l)),
            [false, true, false]
        );
        sched.ul_reserve_grant(RADIO, vec![subslot], true, None);
        assert_eq!(
            [SchF, SchHu].map(|l| sched.ul_reserved_as(subslot, RADIO, l)),
            [true, false],
            "two subslots"
        );
    }

    fn time(m: u8, f: u8, t: u8) -> TdmaTime {
        TdmaTime { t, f, m, h: 0 }
    }

    /// The MS's reading of a slot grant received in downlink slot `grant_dl` on a channel of the
    /// slots `channel` (EN 300 392-2 23.5.2.2.2, 23.5.2.2.4), written apart from the scheduler:
    /// the same-numbered uplink slot is opportunity 0; `delay` more slots of the channel, frame 18
    /// included; then `slots` successive slots of the channel, jumping the common linearization
    /// ones of frame 18.
    fn ms_granted_labels(grant_dl: TdmaTime, delay: usize, slots: usize, channel: [bool; 4]) -> Vec<TdmaTime> {
        let next = |mut t: TdmaTime| loop {
            t = t.add_timeslots(1);
            if channel[t.t as usize - 1] {
                return t;
            }
        };
        let mut t = grant_dl;
        for _ in 0..delay {
            t = next(t);
        }
        let mut out = Vec::new();
        while out.len() < slots {
            if !t.is_mandatory_clch() {
                out.push(t);
            }
            t = next(t);
        }
        out
    }

    /// The MAC-RESOURCEs of the SCH/F block of a slot.
    fn resources(slot: &TmvUnitdataReqSlot) -> Vec<MacResource> {
        let Some(blk) = slot.blk1.as_ref().filter(|b| b.logical_channel == LogicalChannel::SchF) else {
            return Vec::new();
        };
        let mut block = blk.mac_block.clone();
        block.seek(0);
        let bits = block.to_bitstr();
        let mut out = Vec::new();
        let mut pos = 0;
        while pos + 16 <= bits.len() {
            let mut b = BitBuffer::from_bitstr(&bits[pos..]);
            b.seek(0);
            if b.peek_bits(2) != Some(0) {
                break;
            }
            let Ok(res) = MacResource::from_bitbuf(&mut b) else { break };
            if res.addr.is_none() {
                break;
            }
            let li = res.length_ind;
            out.push(res);
            if li == 0b111111 || li == 0 {
                break;
            }
            pos += usize::from(li) * 8;
        }
        out
    }

    /// The slot grant a slot carries for the radio: (slots, delay).
    fn grant_in(slot: &TmvUnitdataReqSlot) -> Option<(usize, usize)> {
        let res = resources(slot)
            .into_iter()
            .find(|r| r.addr.is_some_and(|a| a.ssi == RADIO) && r.slot_granting_element.is_some())?;
        let g = res.slot_granting_element?;
        let delay = match g.granting_delay {
            BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity => 0,
            BasicSlotgrantGrantingDelay::DelayNOpportunities(n) => usize::from(n),
            other => panic!("unexpected granting delay {other:?}"),
        };
        Some((g.capacity_allocation.to_req_slotcount(), delay))
    }

    /// Uplink slots reserved to the radio around `from`.
    fn held_by_radio(sched: &BsChannelScheduler, from: TdmaTime) -> Vec<TdmaTime> {
        (-3..=67)
            .map(|k| from.add_timeslots(k))
            .filter(|u| sched.ul_get_slot_owner(*u, PhyBlockNum::Both) == Some(RADIO))
            .collect()
    }

    const CH34: [bool; 4] = [false, false, true, true];

    /// The helper reads the examples of 23.5.2.2.4 as the standard does.
    #[test]
    fn test_the_ms_reading_of_a_grant_follows_the_examples() {
        // EXAMPLE 1: MCCH, 4 slots from slot 1 of frame 16 of multiframe 2.
        assert_eq!(
            ms_granted_labels(time(2, 16, 1), 0, 4, [true, false, false, false]),
            vec![time(2, 16, 1), time(2, 17, 1), time(3, 1, 1), time(3, 2, 1)]
        );
        // EXAMPLE 3: timeslots 3 and 4, four slots from slot 4 of frame 10; six slots from slot 3
        // of frame 17 of multiframe 3 on a pi/4-DQPSK channel.
        assert_eq!(
            ms_granted_labels(time(1, 10, 4), 0, 4, CH34),
            vec![time(1, 10, 4), time(1, 11, 3), time(1, 11, 4), time(1, 12, 3)]
        );
        assert_eq!(
            ms_granted_labels(time(3, 17, 3), 0, 6, CH34),
            vec![
                time(3, 17, 3),
                time(3, 17, 4),
                time(3, 18, 3),
                time(4, 1, 3),
                time(4, 1, 4),
                time(4, 2, 3)
            ]
        );
        // EXAMPLE 4: timeslots 1 and 2, grant in slot 2 of frame 4, delay 5: slot 1 of frame 7.
        assert_eq!(
            ms_granted_labels(time(1, 4, 2), 5, 1, [true, true, false, false]),
            vec![time(1, 7, 1)]
        );
    }

    /// A request on the channel is granted in a slot of the channel it hears, with the delay and
    /// the slots counted over the channel: the slots the MS will use are the ones reserved to it,
    /// and only those.
    #[test]
    fn test_multislot_grant_is_counted_over_the_channel() {
        let mut sched = channel_sched(&[3, 4]);
        assert!(sched.ul_defer_to_channel(at(2, 4), radio(), &ReservationRequirement::Req4Slots));
        let mut grants = Vec::new();
        for _ in 0..16 {
            let slot = finalize_slots(&mut sched, 1).remove(0);
            if let Some((n, delay)) = grant_in(&slot) {
                let labels = ms_granted_labels(slot.ts, delay, n, CH34);
                assert_eq!(held_by_radio(&sched, slot.ts), labels);
                grants.push((slot.ts, n, delay));
            }
        }
        assert_eq!(grants, vec![(at(4, 3), 4, 0)]);
    }

    /// Anywhere else a request goes the usual way: on the MCCH, on a channel of one slot, from
    /// another radio.
    #[test]
    fn test_a_request_off_the_channel_goes_the_usual_way() {
        let mut sched = channel_sched(&[3, 4]);
        let req = ReservationRequirement::Req2Slots;
        assert!(!sched.ul_defer_to_channel(at(2, 1), radio(), &req));
        assert!(!sched.ul_defer_to_channel(at(2, 3), TetraAddress::issi(RADIO + 1), &req));
        let mut single = channel_sched(&[4]);
        assert!(!single.ul_defer_to_channel(at(2, 4), radio(), &req));
        assert!(sched.pdch_channel_debt.iter().all(Option::is_none));
    }

    /// EN 300 392-2 23.5.2.2.2: frame 18 slots count as delay opportunities, but a grant never
    /// starts there and a run never takes a frame-18 slot other than a jumped common
    /// linearization one. The request of 6 slots in (16,4) of multiframe 1 (CLCH (18,2)) on
    /// {2,3,4} is granted 4 in (17,4) with delay 4: (1,2), (1,3), (1,4), (2,2); 2 stay owed.
    #[test]
    fn test_multislot_grant_never_starts_in_frame_18_and_counts_it() {
        let mut sched = multislot_slotter(RADIO, &[2, 3, 4]);
        sched.set_dl_time(at(17, 1));
        finalize_slots(&mut sched, 1);
        assert_eq!(sched.cur_dltime, at(17, 2));
        assert!(sched.ul_defer_to_channel(at(16, 4), radio(), &ReservationRequirement::Req6Slots));
        let slot = finalize_slots(&mut sched, 1).remove(0);
        assert_eq!(slot.ts, at(17, 4));
        assert_eq!(grant_in(&slot), Some((4, 4)));
        let labels = vec![time(2, 1, 2), time(2, 1, 3), time(2, 1, 4), time(2, 2, 2)];
        assert_eq!(ms_granted_labels(slot.ts, 4, 4, [false, true, true, true]), labels);
        assert_eq!(held_by_radio(&sched, slot.ts), labels);
        let debt = sched.pdch_channel_debt[1].expect("2 owed");
        assert_eq!((debt.slots, debt.not_before), (2, time(2, 2, 2)));
    }

    /// Its debt is not granted in a slot the radio does not hear (G1) but in the next one it does.
    #[test]
    fn test_multislot_grant_is_not_sent_in_a_slot_the_radio_cannot_hear() {
        let mut sched = multislot_slotter(RADIO, &[3, 4]);
        // The slot built next is (5,4).
        sched.set_dl_time(at(5, 2));
        sched.ul_reserve_grant(RADIO, vec![at(5, 3)], false, None);
        assert!(sched.ul_defer_to_channel(at(5, 3), radio(), &ReservationRequirement::Req1Slot));
        let slots = finalize_slots(&mut sched, 4);
        assert_eq!((slots[0].ts, grant_in(&slots[0])), (at(5, 4), None));
        assert_eq!(slots[3].ts, at(6, 3));
        assert_eq!(grant_in(&slots[3]).map(|g| g.0), Some(1));
    }

    /// No delay above 13 is ever sent: with the first 16 opportunities taken nothing is granted
    /// and the debt waits; freed, the next slot of the channel grants it.
    #[test]
    fn test_multislot_grant_waits_for_a_delay_of_13() {
        let mut sched = channel_sched(&[3, 4]);
        let busy: Vec<TdmaTime> = (0..16u8).map(|i| at(4 + i / 2, 3 + i % 2)).collect();
        sched.ul_reserve_grant(RADIO + 1, busy.clone(), false, None);
        assert!(sched.ul_defer_to_channel(at(3, 3), radio(), &ReservationRequirement::Req1Slot));
        assert!(finalize_slots(&mut sched, 4).iter().all(|s| grant_in(s).is_none()));
        assert!(sched.pdch_channel_debt[2].is_some(), "the debt waits");
        for u in busy {
            sched.ul_free_slot(u);
        }
        let slots = finalize_slots(&mut sched, 4);
        assert_eq!((slots[2].ts, grant_in(&slots[2])), (at(5, 3), Some((1, 0))));
    }

    /// A large requirement is granted in chunks of up to four slots, each after the slots of the
    /// previous one, every one of them reserved as the MS will use it.
    #[test]
    fn test_multislot_debt_is_granted_in_chunks() {
        let mut sched = channel_sched(&[3, 4]);
        assert!(sched.ul_defer_to_channel(at(3, 3), radio(), &ReservationRequirement::Req13Slots));
        let mut chunks = Vec::new();
        let mut last: Option<TdmaTime> = None;
        for _ in 0..4 * 40 {
            let slot = finalize_slots(&mut sched, 1).remove(0);
            if let Some((n, delay)) = grant_in(&slot) {
                let labels = ms_granted_labels(slot.ts, delay, n, CH34);
                for u in &labels {
                    assert_eq!(sched.ul_get_slot_owner(*u, PhyBlockNum::Both), Some(RADIO), "{u}");
                }
                if let Some(prev) = last {
                    assert!(labels[0].diff(prev) > 0, "after the previous chunk");
                }
                last = labels.last().copied();
                chunks.push(n);
            }
        }
        assert_eq!(chunks, vec![4, 4, 4, 1]);
    }

    /// The requirement is the radio's whole estimate: the slots it already holds ahead on its
    /// channel are not owed again (23.5.2.1 NOTE 1).
    #[test]
    fn test_the_debt_counts_the_slots_already_granted() {
        let mut sched = channel_sched(&[3, 4]);
        sched.ul_reserve_grant(RADIO, vec![at(4, 3), at(4, 4), at(5, 3), at(5, 4)], false, Some(9));
        assert!(sched.ul_defer_to_channel(at(4, 3), radio(), &ReservationRequirement::Req6Slots));
        assert_eq!(sched.pdch_channel_debt[2].map(|d| d.slots), Some(3));
        assert!(sched.ul_defer_to_channel(at(4, 3), radio(), &ReservationRequirement::Req2Slots));
        assert!(sched.pdch_channel_debt[2].is_none(), "nothing more owed");
    }

    /// A PDU saying the radio has nothing more to send frees the slots it still holds after it
    /// (23.5.2.3.1) and drops its debt: the AACH no longer shows them reserved and the radio is no
    /// longer deaf around them.
    #[test]
    fn test_nothing_more_to_send_frees_the_reservations() {
        let mut sched = channel_sched(&[3, 4]);
        sched.ul_reserve_grant(RADIO, vec![at(4, 3), at(4, 4), at(5, 3), at(5, 4)], false, Some(9));
        assert!(sched.ul_defer_to_channel(at(3, 4), radio(), &ReservationRequirement::Req8Slots));
        assert_eq!(sched.pdch_channel_debt[2].map(|d| d.slots), Some(4));
        sched.ul_release_after(at(4, 4), RADIO);
        assert_eq!(held_by_radio(&sched, at(4, 3)), vec![at(4, 3), at(4, 4)]);
        assert!(sched.pdch_channel_debt[2].is_none());
        let slots = finalize_slots(&mut sched, 12);
        let aach_reserved = |s: &TmvUnitdataReqSlot| {
            let mut bbk = s.bbk.as_ref().unwrap().mac_block.clone();
            bbk.seek(0);
            AccessAssign::from_bitbuf(&mut bbk).unwrap().f2_af.map(|af| af.base_frame_len) == Some(0)
        };
        let reserved: Vec<TdmaTime> = slots.iter().filter(|s| aach_reserved(s)).map(|s| s.ts).collect();
        assert_eq!(reserved, vec![at(4, 3), at(4, 4)]);
        // Other radios cannot be released by the radio's PDU.
        sched.ul_reserve_grant(RADIO + 1, vec![at(7, 3)], false, None);
        sched.ul_release_after(at(6, 3), RADIO);
        assert_eq!(sched.ul_get_slot_owner(at(7, 3), PhyBlockNum::Both), Some(RADIO + 1));
    }

    /// A fragmented group PDU under way on the channel holds the grant (G3): it goes in the next
    /// slot of the channel after the group's MAC-END, counted from there.
    #[test]
    fn test_a_group_fragmentation_holds_the_lazy_grant() {
        let mut sched = channel_sched(&[3, 4]);
        let group = TetraAddress::new(91, SsiType::Gssi);
        let sdu: String = (0..600).map(|i| if i % 5 == 0 { '1' } else { '0' }).collect();
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&group, None, false);
        sched.dl_enqueue_tma_for_link(4, pdu, BitBuffer::from_bitstr(&sdu), None);
        let mut slots = finalize_slots(&mut sched, 3);
        assert_eq!(dl_pdus(&slots[2]).first().map(|p| (p.1, p.2)), Some((Some(91), 0b111111)));
        assert!(sched.ul_defer_to_channel(at(4, 3), radio(), &ReservationRequirement::Req2Slots));
        slots.extend(finalize_slots(&mut sched, 8));
        let end = slots.iter().position(|s| dl_pdus(s).iter().any(|p| p.0 == 3)).expect("the MAC-END");
        let granted = slots.iter().position(|s| grant_in(s).is_some()).expect("the grant");
        assert!(granted > end, "the grant after the group's MAC-END ({end}, {granted})");
        let (n, delay) = grant_in(&slots[granted]).unwrap();
        assert_eq!(
            held_by_radio(&sched, slots[granted].ts),
            ms_granted_labels(slots[granted].ts, delay, n, CH34)
        );
    }

    /// At a message boundary the debt goes first: between two datagrams that each need several
    /// slots, the grant leaves before the second starts.
    #[test]
    fn test_a_debt_goes_before_the_next_fragmented_message() {
        let mut sched = channel_sched(&[3, 4]);
        for _ in 0..2 {
            let (pdu, sdu) = resource(2560);
            sched.dl_enqueue_packet_data_for_link(3, pdu, sdu, None);
        }
        let mut slots = finalize_slots(&mut sched, 4);
        assert!(sched.ul_defer_to_channel(at(4, 3), radio(), &ReservationRequirement::Req1Slot));
        slots.extend(finalize_slots(&mut sched, 4 * 16));
        let starts: Vec<usize> = (0..slots.len())
            .filter(|i| dl_pdus(&slots[*i]).iter().any(|p| p.0 == 0 && p.2 == 0b111111))
            .collect();
        assert_eq!(starts.len(), 2, "both datagrams started");
        let granted = slots.iter().position(|s| grant_in(s).is_some()).expect("the grant");
        assert!(starts[0] < granted && granted < starts[1], "{starts:?} {granted}");
    }

    /// While the radio is owed uplink slots, even with none held ahead (between two chunks), no
    /// downlink fragmentation starts: it would hold the next chunk.
    #[test]
    fn test_no_downlink_fragmentation_between_two_uplink_chunks() {
        let mut sched = channel_sched(&[3, 4]);
        assert!(sched.ul_defer_to_channel(at(3, 3), radio(), &ReservationRequirement::Req13Slots));
        let (pdu, sdu) = resource(1200);
        sched.dl_enqueue_packet_data_for_link(3, pdu, sdu, None);
        let mut last_granted: Option<TdmaTime> = None;
        let mut start = None;
        for _ in 0..4 * 40 {
            let slot = finalize_slots(&mut sched, 1).remove(0);
            if let Some((n, delay)) = grant_in(&slot) {
                last_granted = ms_granted_labels(slot.ts, delay, n, CH34).last().copied();
            }
            if start.is_none() && dl_pdus(&slot).iter().any(|p| p.0 == 0 && p.2 == 0b111111) {
                start = Some(slot.ts);
                assert!(sched.pdch_channel_debt[2].is_none(), "nothing owed any more");
            }
        }
        let (start, last) = (start.expect("the datagram started"), last_granted.expect("granted"));
        assert!(start.add_timeslots(-3).diff(last) > 0, "after the last chunk: {start} {last}");
    }

    /// A grant placed in a slot that then did not carry it (here a fragment put ahead of it) is
    /// taken back: its slots free, the debt as before, the PDU without it.
    #[test]
    fn test_a_lazy_grant_that_did_not_leave_is_undone() {
        let mut sched = channel_sched(&[3, 4]);
        assert!(sched.ul_defer_to_channel(at(3, 3), radio(), &ReservationRequirement::Req2Slots));
        let (pdu, sdu) = resource(64);
        sched.dl_enqueue_tma_for_link(3, pdu, sdu, None);
        finalize_slots(&mut sched, 2);
        let d = sched.cur_dltime.add_timeslots(2);
        sched.tick_start(sched.cur_dltime.add_timeslots(1));
        assert_eq!(d, at(4, 3));
        sched.pdch_lazy_grant(d);
        assert_eq!(held_by_radio(&sched, d), vec![at(4, 3), at(4, 4)]);
        let (other, long) = resource(800);
        let mut fragger = BsFragger::new(other, long, None);
        assert!(!fragger.get_next_chunk(&mut BitBuffer::new(SCH_F_CAP)));
        sched.dltx_queues[2].insert(0, DlSchedElem::FragBuf(fragger));
        sched.dl_build_block_from_signalling_schedule(d);
        sched.pdch_check_lazy_grant(d);
        assert!(held_by_radio(&sched, d).is_empty(), "its slots freed");
        assert_eq!(sched.pdch_channel_debt[2].map(|debt| debt.slots), Some(2), "owed again");
        // With no room left it was queued again as a fragger none of which went out.
        let stripped = sched.dltx_queues[2].iter_mut().any(|e| match e {
            DlSchedElem::Resource(pdu, ..) => pdu.addr == Some(radio()) && pdu.slot_granting_element.is_none(),
            DlSchedElem::FragBuf(f) => f
                .unsent_resource_mut()
                .is_some_and(|pdu| pdu.addr == Some(radio()) && pdu.slot_granting_element.is_none()),
            _ => false,
        });
        assert!(stripped, "the PDU is there without the grant");
    }

    /// G2 is decided after the grant is placed: the slot carrying the grant of a radio owed one
    /// slot does not start the datagram queued behind it.
    #[test]
    fn test_g2_is_evaluated_after_the_lazy_reservation() {
        let mut sched = channel_sched(&[3, 4]);
        assert!(sched.ul_defer_to_channel(at(3, 3), radio(), &ReservationRequirement::Req1Slot));
        let (pdu, sdu) = resource(600);
        sched.dl_enqueue_packet_data_for_link(3, pdu, sdu, None);
        let slots = finalize_slots(&mut sched, 3);
        assert_eq!(slots[2].ts, at(4, 3));
        assert_eq!(grant_in(&slots[2]), Some((1, 0)));
        assert!(!dl_pdus(&slots[2]).iter().any(|p| p.2 == 0b111111), "{:?}", dl_pdus(&slots[2]));
    }

    /// A full advanced link segment (a 211-bit TM-SDU) with a grant of several slots and its usage
    /// marker fills the block to the bit: it goes whole, the grant with it, and the segments after
    /// it follow in order.
    #[test]
    fn test_a_grant_rides_on_a_segment_that_fills_the_block() {
        let mut sched = channel_sched(&[3, 4]);
        let reporters: Vec<TxReporter> = (0..3).map(|_| TxReporter::new()).collect();
        for r in &reporters {
            let (pdu, sdu) = resource(211);
            sched.dl_enqueue_tma_for_link(3, pdu, sdu, Some(r.clone()));
        }
        assert!(sched.ul_defer_to_channel(at(3, 3), radio(), &ReservationRequirement::Req4Slots));
        let slots = finalize_slots(&mut sched, 16);
        let carried: Vec<(TdmaTime, Option<(usize, usize)>)> = slots
            .iter()
            .filter(|s| dl_pdus(s).iter().any(|p| p.1 == Some(RADIO)))
            .map(|s| (s.ts, grant_in(s)))
            .collect();
        assert_eq!(carried, vec![(at(4, 3), Some((4, 0))), (at(6, 4), None), (at(7, 3), None)]);
        assert!(reporters.iter().all(|r| r.is_transmitted()));
        assert!(sched.pdch_channel_debt[2].is_none(), "nothing owed any more");
    }

    /// When the channel goes, what was queued on it for its radio and what it was owed go too.
    #[test]
    fn test_losing_a_multislot_channel_drops_its_queue() {
        let mut sched = channel_sched(&[3, 4]);
        let reporter = TxReporter::new();
        let (pdu, sdu) = resource(64);
        sched.dl_enqueue_tma_for_link(4, pdu, sdu, Some(reporter.clone()));
        sched.ul_reserve_grant(RADIO, vec![at(5, 3)], false, None);
        assert!(sched.ul_defer_to_channel(at(3, 3), radio(), &ReservationRequirement::Req4Slots));
        sched.pdch_want_reply(RADIO, reporter.clone());
        sched.set_pdch(3, None);
        sched.set_pdch(4, None);
        assert_eq!(reporter.get_state(), tetra_core::TxState::Discarded);
        assert!(sched.dltx_queues.iter().all(|q| q.is_empty()));
        assert!(sched.pdch_channel_debt.iter().all(Option::is_none));
        assert!(sched.pdch_reply_wanted.is_empty());
    }

    /// A segment that asks for an acknowledgement leaves with a grant of one full slot for the
    /// answer, far enough for the MS to build it; the radio is then deaf around that slot. The
    /// segments before it and after it get none.
    #[test]
    fn test_a_reply_slot_rides_on_an_ar_segment() {
        let mut sched = channel_sched(&[3, 4]);
        let reporters: Vec<TxReporter> = (0..5).map(|_| TxReporter::new_unacked()).collect();
        for (i, r) in reporters.iter().enumerate() {
            let (pdu, sdu) = resource(200);
            sched.dl_enqueue_tma_for_link(4, pdu, sdu, Some(r.clone()));
            if i == 1 {
                sched.pdch_want_reply(RADIO, r.clone());
            }
        }
        let slots = finalize_slots(&mut sched, 16);
        let carried: Vec<(TdmaTime, Option<(usize, usize)>)> = slots
            .iter()
            .filter(|s| dl_pdus(s).iter().any(|p| p.1 == Some(RADIO)))
            .map(|s| (s.ts, grant_in(s)))
            .collect();
        // The second goes in (4,4) with its reply slot (5,4), two opportunities on; the radio does
        // not hear (6,3), so the fifth goes in (6,4).
        assert_eq!(
            carried,
            vec![
                (at(4, 3), None),
                (at(4, 4), Some((1, 2))),
                (at(5, 3), None),
                (at(5, 4), None),
                (at(6, 4), None)
            ]
        );
        assert!(reporters.iter().all(|r| r.is_transmitted()));
        assert!(sched.pdch_reply_wanted.is_empty(), "the wish went with its segment");
    }

    /// A request the radio sends by random access in uplink u is not taken as covered by a slot
    /// granted in downlink u+1..u+3, which it did not hear while it transmitted: that slot is
    /// freed and what it asks for is granted afresh.
    #[test]
    fn test_a_random_access_does_not_count_grants_the_radio_did_not_hear() {
        let mut sched = channel_sched(&[3, 4]);
        let reporter = TxReporter::new_unacked();
        let (pdu, sdu) = resource(200);
        sched.dl_enqueue_tma_for_link(4, pdu, sdu, Some(reporter.clone()));
        sched.pdch_want_reply(RADIO, reporter);
        let slots = finalize_slots(&mut sched, 3);
        assert_eq!((slots[2].ts, grant_in(&slots[2])), (at(4, 3), Some((1, 2))));
        assert_eq!(held_by_radio(&sched, at(4, 3)), vec![at(5, 3)]);
        // A MAC-ACCESS asking for a slot came by random access in (3,4), learnt of now.
        let label = at(3, 4);
        assert_eq!(sched.cur_dltime.add_timeslots(-2), label);
        sched.pdch_random_access(label, PhyBlockNum::Block1, RADIO);
        assert!(sched.ul_defer_to_channel(label, radio(), &ReservationRequirement::Req1Slot));
        assert!(held_by_radio(&sched, label).is_empty(), "the reply slot it did not hear freed");
        assert_eq!(sched.pdch_channel_debt[2].map(|d| d.slots), Some(1));
        let slot = finalize_slots(&mut sched, 1).remove(0);
        assert_eq!((slot.ts, grant_in(&slot)), (at(4, 4), Some((1, 0))));
        // Not a grant sent once it was back (u+4 on).
        sched.pdch_random_access(label, PhyBlockNum::Block1, RADIO);
        assert_eq!(held_by_radio(&sched, label), vec![at(4, 4)]);
    }

    /// A random access of the radio on its channel drops the downlink fragmentation under way on
    /// it (its reporter says discarded); a PDU in a slot reserved to it does not.
    #[test]
    fn test_a_random_access_aborts_the_downlink_fragmentation() {
        let mut sched = channel_sched(&[3, 4]);
        let reporter = TxReporter::new();
        let (pdu, sdu) = resource(1000);
        sched.dl_enqueue_packet_data_for_link(3, pdu, sdu, Some(reporter.clone()));
        finalize_slots(&mut sched, 4);
        let started = |sched: &BsChannelScheduler| {
            sched.dltx_queues[2]
                .iter()
                .any(|e| matches!(e, DlSchedElem::FragBuf(f) if f.is_started()))
        };
        assert!(started(&sched), "the datagram is being fragmented");
        let label = sched.cur_dltime.add_timeslots(-2).forward_to_timeslot(3);
        sched.ul_reserve_grant(RADIO, vec![label], false, None);
        sched.pdch_random_access(label, PhyBlockNum::Both, RADIO);
        assert!(started(&sched), "a reserved slot is no random access");
        sched.pdch_random_access(label.add_timeslots(1), PhyBlockNum::Block1, RADIO);
        assert!(!started(&sched));
        assert_eq!(reporter.get_state(), tetra_core::TxState::Discarded);
    }

    // The packet-data carrier (a carrier without MCCH) ------------------------------------------

    const ALL: [bool; 4] = [true; 4];

    /// The scheduler of the packet-data carrier with the radio's channel `channel`, its next slot
    /// built (4,1).
    fn carrier_sched(channel: &[u8]) -> BsChannelScheduler {
        let mut sched = carrier_slotter(RADIO, channel);
        sched.set_dl_time(at(3, 3));
        sched
    }

    /// The AACH of a slot: (header, ACCESS-ASSIGN).
    fn aach(slot: &TmvUnitdataReqSlot) -> (u8, AccessAssign) {
        let mut bbk = slot.bbk.as_ref().unwrap().mac_block.clone();
        bbk.seek(0);
        let header = bbk.peek_bits(2).unwrap() as u8;
        (header, AccessAssign::from_bitbuf(&mut bbk).unwrap())
    }

    fn one_slot_grant() -> BasicSlotgrant {
        BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::Grant1Slot,
            granting_delay: BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity,
        }
    }

    /// Ts1 is a traffic slot of a carrier without MCCH: a channel may take it there, never on
    /// the main carrier.
    #[test]
    fn test_a_carrier_without_mcch_takes_a_pdch_on_ts1() {
        let sched = carrier_slotter(RADIO, &[1, 2, 3, 4]);
        assert_eq!(sched.pdch_timeslots_of(RADIO), ALL);
        assert_eq!((sched.pdch_anchor(3, RADIO), sched.queue_index(4)), (Some(1), 0));
        let main = multislot_slotter(RADIO, &[1, 2]);
        assert_eq!(
            (main.pdch_owner(1), main.pdch_owner(2)),
            (None, Some(RADIO)),
            "ts1 of the main carrier is the MCCH"
        );
    }

    /// Every slot of a carrier channel shows assigned control (Header 2); the uplink of ts1 always
    /// shows reserved, so the radio never random-accesses there (23.5.1.4.2 b). Without a channel
    /// ts1 is unallocated as always.
    #[test]
    fn test_carrier_pdch_ts1_aach_is_assigned_control_and_reserved() {
        let mut sched = carrier_sched(&[1, 2, 3, 4]);
        for slot in finalize_slots(&mut sched, 4 * 20).iter().filter(|s| s.ts.f != 18) {
            let (header, a) = aach(slot);
            assert_eq!(
                (header, a.dl_usage, a.ul_usage),
                (2, AccessAssignDlUsage::AssignedControl, AccessAssignUlUsage::AssignedOnly),
                "{}",
                slot.ts
            );
            let reserved = a.f2_af.map(|af| af.base_frame_len) == Some(0);
            assert_eq!(reserved, slot.ts.t == 1, "{}", slot.ts);
        }
        let mut idle = carrier_sched(&[]);
        for slot in finalize_slots(&mut idle, 8).iter().filter(|s| s.ts.t == 1) {
            let (_, a) = aach(slot);
            assert_eq!(
                (a.dl_usage, a.ul_usage),
                (AccessAssignDlUsage::Unallocated, AccessAssignUlUsage::Unallocated)
            );
        }
    }

    /// The uplink of a carrier channel never takes ts1: the MS counts it as an opportunity
    /// (23.5.2.2.2), the BS never grants it, so a run ends there and a chunk has three slots at
    /// most; the slots reserved are the ones the MS will use.
    #[test]
    fn test_no_uplink_grant_on_carrier_ts1() {
        for (start, req, total) in [
            (at(3, 3), ReservationRequirement::Req6Slots, 6),
            (at(15, 3), ReservationRequirement::Req13Slots, 13),
        ] {
            assert_eq!(granted_on_a_carrier_channel(start, req).len(), total, "{start}");
        }
    }

    /// The uplink slots the radio of a carrier channel {1,2,3,4} reads in the grants for `req`
    /// asked just before `start`, each checked: never ts1, three at most per grant, every one
    /// reserved to it by the BS.
    fn granted_on_a_carrier_channel(start: TdmaTime, req: ReservationRequirement) -> Vec<TdmaTime> {
        let mut sched = carrier_slotter(RADIO, &[1, 2, 3, 4]);
        sched.set_dl_time(start);
        assert!(sched.ul_defer_to_channel(start.add_timeslots(-1), radio(), &req));
        let mut granted = Vec::new();
        for _ in 0..4 * 40 {
            let slot = finalize_slots(&mut sched, 1).remove(0);
            if let Some((n, delay)) = grant_in(&slot) {
                let labels = ms_granted_labels(slot.ts, delay, n, ALL);
                assert!(n <= 3 && labels.iter().all(|u| u.t != 1), "{} {labels:?}", slot.ts);
                for u in &labels {
                    assert_eq!(sched.ul_get_slot_owner(*u, PhyBlockNum::Both), Some(RADIO), "{u}");
                }
                granted.extend(labels);
            }
        }
        granted
    }

    /// Grants on four slots across frame 18, as the MS counts them (frame 18 included, its common
    /// linearization slot jumped): in a multiframe where that slot is ts1 ((MN + 1) mod 4 = 3) and
    /// in one where it is ts3, every slot the MS uses is the one reserved to it.
    #[test]
    fn test_multislot_grant_is_counted_over_four_slots() {
        for m in [2u8, 4] {
            let start = time(m, 15, 3);
            let granted = granted_on_a_carrier_channel(start, ReservationRequirement::Req13Slots);
            assert_eq!(granted.len(), 13, "{start}: {granted:?}");
            assert!(granted.iter().all(|u| u.f != 18), "{start}: {granted:?}");
        }
    }

    /// Downlink ts1 of a carrier channel carries its owner's grants, acknowledgements and PDUs;
    /// another radio's are dropped there as before.
    #[test]
    fn test_grant_ra_ack_and_link_on_carrier_dl_ts1_for_its_owner() {
        let mut sched = carrier_sched(&[1, 2, 3, 4]);
        let other = TetraAddress::issi(RADIO + 1);
        sched.dl_enqueue_random_access_ack(1, radio());
        sched.dl_enqueue_random_access_ack(1, other);
        sched.dl_enqueue_grant(1, other, one_slot_grant(), None);
        let (pdu, sdu) = resource(64);
        sched.dl_enqueue_tma_for_link(1, pdu, sdu, None);
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&other, None, false);
        sched.dl_enqueue_tma_for_link(1, pdu, BitBuffer::from_bitstr("1010"), None);
        assert_eq!(sched.dltx_queues[0].len(), 2, "the owner's only");
        let slot = finalize_slots(&mut sched, 1).remove(0);
        assert_eq!(slot.ts, at(4, 1));
        assert!(dl_pdus(&slot).iter().all(|p| p.1 == Some(RADIO)), "{:?}", dl_pdus(&slot));
        assert!(!dl_pdus(&slot).is_empty());
    }

    /// A radio assigned its carrier channel in main (4,1) (23.5.4.3.1 rule 1 a): on {1,2,3,4} its
    /// first downlink slot is (4,3) and it linearises in uplink (4,1), deaf until (4,4), so the
    /// first PDU goes in (5,1); on {3,4} in (4,3), then not in (4,4); on {2} in (5,2).
    #[test]
    fn test_the_channel_is_entered_per_rule_1a_with_linearisation() {
        for (channel, first, silent) in [
            (&[1u8, 2, 3, 4][..], at(5, 1), None),
            (&[3u8, 4][..], at(4, 3), Some(at(4, 4))),
            (&[2u8][..], at(5, 2), None),
        ] {
            let mut sched = carrier_sched(channel);
            let mut bits = [false; 4];
            for ts in channel {
                bits[usize::from(*ts) - 1] = true;
            }
            sched.pdch_note_assignment_from_mcch(RADIO, bits, at(4, 1));
            for _ in 0..8 {
                let (pdu, sdu) = resource(64);
                sched.dl_enqueue_tma_for_link(u32::from(channel[0]), pdu, sdu, None);
            }
            let carried: Vec<TdmaTime> = finalize_slots(&mut sched, 8)
                .iter()
                .filter(|s| dl_pdus(s).iter().any(|p| p.1 == Some(RADIO)))
                .map(|s| s.ts)
                .collect();
            assert_eq!(carried.first(), Some(&first), "{channel:?}: {carried:?}");
            assert!(silent.is_none_or(|t| !carried.contains(&t)), "{channel:?}: {carried:?}");
        }
    }

    /// Frame 18 of a carrier channel (9.5.2): BSCH + BNCH only where (MN + TN) mod 4 = 3, SCH/HD
    /// + BNCH where it is 1, SCH/F elsewhere. A main-carrier channel and an idle carrier slot keep
    /// BSCH + BNCH.
    #[test]
    fn test_frame_18_on_a_carrier_pdch_follows_9_5_2() {
        for m in 1..=4u8 {
            let mut sched = carrier_slotter(RADIO, &[1, 2, 3, 4]);
            sched.set_dl_time(TdmaTime { t: 3, f: 17, m, h: 0 });
            let slots = finalize_slots(&mut sched, 6);
            let f18: Vec<&TmvUnitdataReqSlot> = slots.iter().filter(|s| s.ts.f == 18).collect();
            assert_eq!(f18.len(), 4);
            for s in f18 {
                let expected = match (s.ts.m + s.ts.t) % 4 {
                    3 => LogicalChannel::Bsch,
                    1 => LogicalChannel::SchHd,
                    _ => LogicalChannel::SchF,
                };
                assert_eq!(s.blk1.as_ref().unwrap().logical_channel, expected, "{}", s.ts);
                let second = s.blk2.as_ref().map(|b| b.logical_channel);
                assert_eq!(
                    second,
                    (expected != LogicalChannel::SchF).then_some(LogicalChannel::Bnch),
                    "{}",
                    s.ts
                );
            }
        }
        let mut main = multislot_slotter(RADIO, &[2, 3, 4]);
        let mut idle = carrier_slotter(RADIO, &[]);
        for sched in [&mut main, &mut idle] {
            sched.set_dl_time(TdmaTime { t: 3, f: 17, m: 1, h: 0 });
            for s in finalize_slots(sched, 6).iter().filter(|s| s.ts.f == 18) {
                assert_eq!(s.blk1.as_ref().unwrap().logical_channel, LogicalChannel::Bsch, "{}", s.ts);
            }
        }
    }

    /// With a packet-data carrier in use, no grant rides on an assignment to it (the move would
    /// wait for the granted slots, 23.5.4.3.1 rule 1 b): it goes on its own, and the assignment is
    /// listed for the hand-off. Without one the grant is merged as always and nothing is listed.
    #[test]
    fn test_no_grant_is_merged_into_a_cross_carrier_assignment() {
        use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
        use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
        for handoff in [None, Some(1002)] {
            let mut sched = get_testing_slotter();
            sched.set_pdch_handoff_carrier(handoff);
            sched.set_pdch_handoff_owners([Some(RADIO); 4]);
            sched.set_dl_time(at(3, 3));
            let mut assignment = BsChannelScheduler::dl_make_minimal_resource(&radio(), None, false);
            assignment.chan_alloc_element = Some(ChanAllocElement {
                alloc_type: ChanAllocType::Replace,
                ts_assigned: ALL,
                ul_dl_assigned: UlDlAssignment::Both,
                clch_permission: true,
                cell_change_flag: false,
                carrier_num: 1002,
                ext: None,
                mon_pattern: 1,
                frame18_mon_pattern: None,
            });
            sched.dl_enqueue_tma_for_link(0, assignment, BitBuffer::from_bitstr("1011001110001111"), None);
            sched.dl_enqueue_grant(1, radio(), one_slot_grant(), None);
            let slot = finalize_slots(&mut sched, 1).remove(0);
            assert_eq!(slot.ts, at(4, 1));
            let mine: Vec<(bool, bool)> = resources(&slot)
                .iter()
                .filter(|r| r.addr.is_some_and(|a| a.ssi == RADIO))
                .map(|r| (r.chan_alloc_element.is_some(), r.slot_granting_element.is_some()))
                .collect();
            let listed = sched.take_assignments_elsewhere();
            match handoff {
                None => {
                    assert_eq!(mine, vec![(true, true)], "merged as always");
                    assert!(listed.is_empty());
                }
                Some(_) => {
                    assert_eq!(mine, vec![(true, false), (false, true)], "the grant on its own");
                    assert_eq!(listed, vec![(RADIO, ALL, at(4, 1))]);
                }
            }
        }
    }

    /// The hand-off: the radio's MCCH reservations after its assignment are freed (another
    /// radio's stay), its grant dropped, its queued PDUs go to its channel queue on the carrier in
    /// their order; the group's stay on the MCCH, and so does a copy of its assignment (told by
    /// its channel allocation being the radio's channel on the carrier).
    #[test]
    fn test_the_handoff_frees_mcch_reservations_and_moves_items() {
        use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
        use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
        let mut main = get_testing_slotter();
        main.set_pdch_handoff_carrier(Some(1002));
        main.set_pdch_handoff_owners([Some(RADIO); 4]);
        main.set_dl_time(at(3, 3));
        let mut assignment = BsChannelScheduler::dl_make_minimal_resource(&radio(), None, false);
        assignment.chan_alloc_element = Some(ChanAllocElement {
            alloc_type: ChanAllocType::Replace,
            ts_assigned: ALL,
            ul_dl_assigned: UlDlAssignment::Both,
            clch_permission: true,
            cell_change_flag: false,
            carrier_num: 1002,
            ext: None,
            mon_pattern: 1,
            frame18_mon_pattern: None,
        });
        main.dl_enqueue_tma_for_link(0, assignment, BitBuffer::from_bitstr("1011001110001111"), None);
        main.ul_reserve_grant(RADIO, vec![at(5, 1)], false, None);
        main.ul_reserve_grant(RADIO + 1, vec![at(6, 1)], false, None);
        let ack = BsChannelScheduler::dl_make_minimal_resource(&radio(), None, false);
        main.dl_enqueue_tma_for_link(0, ack, BitBuffer::from_bitstr("0110"), None);
        let (pdu, sdu) = resource(40);
        main.dl_enqueue_tma_for_link(0, pdu, sdu, None);
        main.dl_enqueue_grant(1, radio(), one_slot_grant(), None);
        let group = TetraAddress::new(91, SsiType::Gssi);
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&group, None, false);
        main.dl_enqueue_tma_for_link(0, pdu, BitBuffer::from_bitstr("1111"), None);
        assert_eq!(main.ul_release_for_handoff(at(4, 1), RADIO), 1);
        assert_eq!(main.ul_get_slot_owner(at(5, 1), PhyBlockNum::Both), None);
        assert_eq!(main.ul_get_slot_owner(at(6, 1), PhyBlockNum::Both), Some(RADIO + 1));
        let (items, dropped) = main.take_individual_items(RADIO);
        assert_eq!((items.len(), dropped), (2, 1));
        assert_eq!(main.dltx_queues[0].len(), 2, "the assignment and the group's stay");
        let mut carrier = carrier_sched(&[1, 2, 3, 4]);
        carrier.enqueue_on_pdch(RADIO, items);
        let sizes: Vec<usize> = carrier.dltx_queues[0]
            .iter()
            .map(|e| match e {
                DlSchedElem::Resource(_, sdu, ..) => sdu.get_len(),
                _ => 0,
            })
            .collect();
        assert_eq!(sizes, vec![4, 40]);
    }

    /// A channel allocation that is not a packet-data channel assignment (a call's, say) still
    /// carries the radio's grant with a packet-data carrier in use.
    #[test]
    fn test_a_call_allocation_to_the_carrier_still_carries_the_grant() {
        use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
        use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
        let mut sched = get_testing_slotter();
        sched.set_pdch_handoff_carrier(Some(1002));
        sched.set_pdch_handoff_owners([None, Some(RADIO), Some(RADIO), Some(RADIO)]);
        sched.set_dl_time(at(3, 3));
        let mut setup = BsChannelScheduler::dl_make_minimal_resource(&radio(), None, false);
        setup.chan_alloc_element = Some(ChanAllocElement {
            alloc_type: ChanAllocType::Replace,
            ts_assigned: [true, false, false, false],
            ul_dl_assigned: UlDlAssignment::Both,
            clch_permission: true,
            cell_change_flag: false,
            carrier_num: 1002,
            ext: None,
            mon_pattern: 1,
            frame18_mon_pattern: None,
        });
        sched.dl_enqueue_tma_for_link(0, setup, BitBuffer::from_bitstr("1011001110001111"), None);
        sched.dl_enqueue_grant(1, radio(), one_slot_grant(), None);
        let slot = finalize_slots(&mut sched, 1).remove(0);
        let mine: Vec<(bool, bool)> = resources(&slot)
            .iter()
            .filter(|r| r.addr.is_some_and(|a| a.ssi == RADIO))
            .map(|r| (r.chan_alloc_element.is_some(), r.slot_granting_element.is_some()))
            .collect();
        assert_eq!(mine, vec![(true, true)]);
    }

    /// A packet-data channel assignment that is no longer its radio's channel when it goes out (a
    /// call took a slot, the channel went or is now another radio's, while it waited in the queue)
    /// goes without its channel allocation, on the main carrier and to the packet-data carrier.
    /// One that still is goes with it, and so do a quit and an allocation not marked (a call's).
    #[test]
    fn test_a_stale_pdch_assignment_goes_without_its_allocation() {
        use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
        use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
        let main = get_testing_slotter().carrier_num;
        let alloc_to = |carrier_num: u16, alloc_type: ChanAllocType, ts_assigned: [bool; 4]| {
            let mut pdu = BsChannelScheduler::dl_make_minimal_resource(&radio(), None, false);
            pdu.chan_alloc_element = Some(ChanAllocElement {
                alloc_type,
                ts_assigned,
                ul_dl_assigned: UlDlAssignment::Both,
                clch_permission: true,
                cell_change_flag: false,
                carrier_num,
                ext: None,
                mon_pattern: 1,
                frame18_mon_pattern: None,
            });
            pdu
        };
        // Whether the PDU went out with its channel allocation.
        let sent_with_it = |mut sched: BsChannelScheduler, pdu: MacResource, marked: bool| {
            sched.set_dl_time(at(3, 3));
            let sdu = BitBuffer::from_bitstr("1011001110001111");
            if marked {
                sched.dl_enqueue_pdch_assignment_for_link(0, pdu, sdu, None);
            } else {
                sched.dl_enqueue_tma_for_link(0, pdu, sdu, None);
            }
            let slot = finalize_slots(&mut sched, 1).remove(0);
            let sent = resources(&slot)
                .into_iter()
                .find(|r| r.addr.is_some_and(|a| a.ssi == RADIO))
                .expect("it went out");
            sent.chan_alloc_element.is_some()
        };
        let own = [false, true, true, true];
        let replace = ChanAllocType::Replace;
        assert!(sent_with_it(
            multislot_slotter(RADIO, &[2, 3, 4]),
            alloc_to(main, replace, own),
            true
        ));
        assert!(
            !sent_with_it(multislot_slotter(RADIO, &[2, 3]), alloc_to(main, replace, own), true),
            "a call took ts4"
        );
        assert!(
            !sent_with_it(multislot_slotter(RADIO + 1, &[2, 3, 4]), alloc_to(main, replace, own), true),
            "another radio's channel now"
        );
        assert!(
            sent_with_it(multislot_slotter(RADIO, &[2, 3]), alloc_to(main, replace, own), false),
            "not marked: as always"
        );
        // The packet-data carrier (1002), its channels as the UMAC last set them.
        let handoff = |owners: [Option<u32>; 4]| {
            let mut sched = get_testing_slotter();
            sched.set_pdch_handoff_carrier(Some(1002));
            sched.set_pdch_handoff_owners(owners);
            sched
        };
        assert!(sent_with_it(handoff([Some(RADIO); 4]), alloc_to(1002, replace, ALL), true));
        assert!(
            !sent_with_it(handoff([None; 4]), alloc_to(1002, replace, ALL), true),
            "the channel went"
        );
        assert!(sent_with_it(
            handoff([None; 4]),
            alloc_to(main, ChanAllocType::QuitAndGo, [false; 4]),
            true
        ));
    }

    /// Only the main carrier's scheduler with a packet-data carrier lists assignments to it:
    /// without one, and on a carrier without MCCH, channel allocations to the other carrier
    /// (calls, a quit to the MCCH) leave nothing to drain.
    #[test]
    fn test_assignments_elsewhere_stay_empty_without_a_carrier() {
        use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
        use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
        let alloc_to = |carrier_num: u16, alloc_type: ChanAllocType, ts_assigned: [bool; 4]| {
            let mut pdu = BsChannelScheduler::dl_make_minimal_resource(&radio(), None, false);
            pdu.chan_alloc_element = Some(ChanAllocElement {
                alloc_type,
                ts_assigned,
                ul_dl_assigned: UlDlAssignment::Both,
                clch_permission: true,
                cell_change_flag: false,
                carrier_num,
                ext: None,
                mon_pattern: 1,
                frame18_mon_pattern: None,
            });
            pdu
        };
        let mut main = get_testing_slotter();
        main.set_dl_time(at(3, 3));
        let mut carrier = carrier_sched(&[2, 3]);
        for _ in 0..25 {
            let pdu = alloc_to(1002, ChanAllocType::Replace, [false, true, false, false]);
            main.dl_enqueue_tma_for_link(0, pdu, BitBuffer::from_bitstr("1011001110001111"), None);
            let pdu = alloc_to(main.carrier_num, ChanAllocType::QuitAndGo, [false; 4]);
            carrier.dl_enqueue_tma_for_link(2, pdu, BitBuffer::from_bitstr("1011001110001111"), None);
            finalize_slots(&mut main, 4);
            finalize_slots(&mut carrier, 4);
        }
        assert!(main.take_assignments_elsewhere().is_empty());
        assert!(carrier.take_assignments_elsewhere().is_empty());
    }

    /// PDUs queued outside a build for the next slot are moved to the MCCH queue before a main
    /// channel slot is built; on a carrier without MCCH queue 0 is a channel's queue and they
    /// stay where they are.
    #[test]
    fn test_out_of_build_items_stay_off_a_carrier_channel() {
        for (mut sched, moved) in [(channel_sched(&[2, 3]), 1), (carrier_sched(&[2, 3]), 0)] {
            let (pdu, sdu) = resource(40);
            sched.dl_enqueue_tma_next_frame(pdu, sdu, None);
            sched.pdch_route_out_of_build_items(at(4, 2));
            assert_eq!(
                (sched.dltx_queues[0].len(), sched.dltx_next_slot_queue.len()),
                (moved, 1 - moved),
                "carrier {}",
                sched.carrier_num
            );
        }
    }

    /// G4: on four slots no downlink fragmentation starts in frames 14 to 17 (it would meet the
    /// four empty slots of frame 18); one started before frame 14 ends before frame 18. On three
    /// slots nothing changes.
    #[test]
    fn test_no_fragmentation_across_frame_18_on_four_slots() {
        let run = |channel: &[u8], carrier: bool, first: TdmaTime| -> (TdmaTime, TdmaTime) {
            let mut sched = if carrier {
                carrier_slotter(RADIO, channel)
            } else {
                multislot_slotter(RADIO, channel)
            };
            sched.set_dl_time(first.add_timeslots(-2));
            let (pdu, sdu) = resource(2000);
            sched.dl_enqueue_packet_data_for_link(u32::from(channel[0]), pdu, sdu, None);
            let slots = finalize_slots(&mut sched, 4 * 10);
            let start = slots.iter().find(|s| dl_pdus(s).iter().any(|p| p.0 == 0 && p.2 == 0b111111));
            let end = slots.iter().find(|s| dl_pdus(s).iter().any(|p| p.0 == 3));
            (start.expect("started").ts, end.expect("ended").ts)
        };
        assert_eq!(run(&[1, 2, 3, 4], true, at(14, 1)).0, time(2, 1, 1));
        let (start, end) = run(&[1, 2, 3, 4], true, at(13, 1));
        assert!(start == at(13, 1) && end.f < 18, "{start} {end}");
        assert_eq!(run(&[2, 3, 4], false, at(14, 2)).0, at(14, 2), "three slots: as before");
    }

    /// On four slots the reply slot of a segment asking for an acknowledgement is four
    /// opportunities away at least, never on ts1 (there, the next opportunity).
    #[test]
    fn test_reply_slot_on_four_slots_is_four_opportunities_away() {
        let mut sched = carrier_sched(&[1, 2, 3, 4]);
        let reporter = TxReporter::new_unacked();
        let (pdu, sdu) = resource(200);
        sched.dl_enqueue_tma_for_link(2, pdu, sdu, Some(reporter.clone()));
        sched.pdch_want_reply(RADIO, reporter);
        let slot = finalize_slots(&mut sched, 4)
            .into_iter()
            .find(|s| grant_in(s).is_some())
            .expect("a reply slot");
        let (n, delay) = grant_in(&slot).unwrap();
        let labels = ms_granted_labels(slot.ts, delay, n, ALL);
        assert_eq!((slot.ts, n, delay), (at(4, 1), 1, 5), "(5,1) is ts1: (5,2)");
        assert_eq!(held_by_radio(&sched, slot.ts), labels);
        assert_eq!(labels, vec![at(5, 2)]);
    }

    /// A carrier slot that stops being a channel sends SCH/F with the AACH unallocated for a
    /// multiframe (9.5.1b NOTE), its previous owner known meanwhile; then BSCH again. On the main
    /// carrier the slot sends what it did (the previous owner is known there too).
    #[test]
    fn test_a_released_carrier_pdch_slot_shows_unallocated_for_a_multiframe() {
        let mut sched = carrier_sched(&[2, 3]);
        finalize_slots(&mut sched, 4);
        let t0 = sched.cur_dltime;
        sched.set_pdch(2, None);
        sched.set_pdch(3, None);
        assert_eq!((sched.pdch_released_owner(2), sched.pdch_released_owner(4)), (Some(RADIO), None));
        for s in finalize_slots(&mut sched, 4 * 20)
            .iter()
            .filter(|s| matches!(s.ts.t, 2 | 3) && s.ts.f != 18)
        {
            let lchan = s.blk1.as_ref().unwrap().logical_channel;
            if s.ts.diff(t0) <= PDCH_RELEASED_UNALLOCATED_SLOTS {
                assert_eq!(lchan, LogicalChannel::SchF, "{}", s.ts);
                assert_eq!(aach(s).1.dl_usage, AccessAssignDlUsage::Unallocated, "{}", s.ts);
            } else {
                assert_eq!(lchan, LogicalChannel::Bsch, "{}", s.ts);
            }
        }
        assert_eq!(sched.pdch_released_owner(2), None, "a multiframe later");
        sched.set_dl_time(t0);
        assert_eq!(sched.pdch_released_owner(2), None, "nor once TDMA time wraps back to it");
        let mut main = channel_sched(&[2, 3]);
        main.set_pdch(2, None);
        assert_eq!(main.pdch_released_owner(2), Some(RADIO));
        let next_ts2 = finalize_slots(&mut main, 4).into_iter().find(|s| s.ts.t == 2).unwrap();
        assert_eq!(next_ts2.blk1.unwrap().logical_channel, LogicalChannel::Bsch);
    }
}
