/// Home Mode Display periodic SDS broadcast — ETSI PID 220.
///
/// Broadcasts a configurable text string to all MSs on the cell (GSSI 0xFFFFFF) at a
/// configurable interval. The MS displays the text on its home screen, typically used
/// to show the base station callsign or network name.
///
/// Ported from Mihajlo Stefanovic (Misadeks) bluestation feature/home-mode-display,
/// adapted for flowstation architecture (no dltime in send_d_sds_data, Layer2Service split).
use tetra_config::bluestation::{HomeModeDisplaySdsTextCodingScheme, SharedConfig};
use tetra_core::{BitBuffer, TdmaTime};
use tetra_saps::control::enums::sds_user_data::SdsUserData;

pub(super) struct HomeModeDisplayTx {
    pub source_issi: u32,
    pub dest_gssi: u32,
    pub payload: SdsUserData,
}

pub(super) struct HomeModeDisplaySender {
    start_time: Option<TdmaTime>,
    last_tx: Option<TdmaTime>,
    warned_invalid_cfg: bool,
    warned_text_truncated: bool,
    warned_lossy_encoding: bool,
    msg_ref: u8,
}

impl HomeModeDisplaySender {
    /// Broadcast destination: all MSs on the cell.
    const HOME_MODE_BROADCAST_GSSI: u32 = 0x00FF_FFFF;
    /// Delay after startup before first broadcast (frames). Avoids congestion during registration.
    const HOME_MODE_START_DELAY_FRAMES: u32 = 96;
    const SLOTS_PER_FRAME: u32 = 4;
    const FRAMES_PER_MULTIFRAME: u32 = 18;
    const SLOTS_PER_MULTIFRAME: u32 = Self::FRAMES_PER_MULTIFRAME * Self::SLOTS_PER_FRAME;
    const MAX_SDS_TYPE4_BYTES: usize = 255; // 2040 bits max when byte-aligned
    const SDS_TL_TRANSFER_HEADER_BYTES: usize = 3;
    const HOME_MODE_TEXT_CODING_SCHEME_BYTES: usize = 1;
    const MAX_TEXT_BYTES: usize =
        Self::MAX_SDS_TYPE4_BYTES - Self::SDS_TL_TRANSFER_HEADER_BYTES - Self::HOME_MODE_TEXT_CODING_SCHEME_BYTES;

    pub fn new() -> Self {
        Self {
            start_time: None,
            last_tx: None,
            warned_invalid_cfg: false,
            warned_text_truncated: false,
            warned_lossy_encoding: false,
            msg_ref: 0,
        }
    }

    /// Called every tick. Returns `Some(HomeModeDisplayTx)` when it's time to broadcast,
    /// `None` otherwise. The caller (SdsBsSubentity) sends the returned PDU via send_d_sds_data.
    pub fn tick_start(&mut self, config: &SharedConfig, dltime: TdmaTime) -> Option<HomeModeDisplayTx> {
        let home_mode_cfg = {
            let cfg = config.config();
            cfg.cell.home_mode_display.clone()
        };

        let Some(home_mode_cfg) = home_mode_cfg else {
            // Feature disabled — reset timers so a re-enable works cleanly
            self.reset_timers();
            self.warned_invalid_cfg = false;
            return None;
        };

        let source_issi = home_mode_cfg.source_issi;
        let interval_multiframes = home_mode_cfg.interval_multiframes;
        let protocol_id = home_mode_cfg.protocol_id;
        let text_coding_scheme = Self::text_coding_scheme_to_sds_tl_value(home_mode_cfg.text_coding_scheme);
        let text = home_mode_cfg.text;

        if interval_multiframes == 0 || source_issi > 0x00FF_FFFF {
            self.reset_timers();
            if !self.warned_invalid_cfg {
                tracing::warn!(
                    "SDS: home_mode_display enabled but invalid config \
                     (source_issi={}, interval_multiframes={}), skipping",
                    source_issi,
                    interval_multiframes
                );
                self.warned_invalid_cfg = true;
            }
            return None;
        }
        self.warned_invalid_cfg = false;

        // Startup delay: wait before first broadcast
        let start_time = self.start_time.get_or_insert(dltime);
        let start_delay_slots =
            Self::HOME_MODE_START_DELAY_FRAMES.saturating_mul(Self::SLOTS_PER_FRAME).min(i32::MAX as u32) as i32;
        if start_time.age(dltime) < start_delay_slots {
            return None;
        }

        // Interval check
        let interval_slots =
            interval_multiframes.saturating_mul(Self::SLOTS_PER_MULTIFRAME).min(i32::MAX as u32) as i32;
        let should_send = match self.last_tx {
            None => true,
            Some(last_tx) => last_tx.age(dltime) >= interval_slots,
        };
        if !should_send {
            return None;
        }

        tracing::info!(
            "SDS: Home Mode Display broadcast src={} dst_gssi=0x{:06X} \
             protocol_id={} interval_multiframes={} text_coding_scheme=0x{:02X}",
            source_issi,
            Self::HOME_MODE_BROADCAST_GSSI,
            protocol_id,
            interval_multiframes,
            text_coding_scheme,
        );

        let (encoded_text, lossy_encoding) = Self::encode_text(&text, text_coding_scheme);
        if lossy_encoding {
            if !self.warned_lossy_encoding {
                tracing::warn!(
                    "SDS: home_mode_display text contains chars unsupported by \
                     coding_scheme=0x{:02X}, replacing with '?'",
                    text_coding_scheme
                );
                self.warned_lossy_encoding = true;
            }
        } else {
            self.warned_lossy_encoding = false;
        }

        let text_len = encoded_text.len().min(Self::MAX_TEXT_BYTES);
        if encoded_text.len() > Self::MAX_TEXT_BYTES {
            if !self.warned_text_truncated {
                tracing::warn!(
                    "SDS: home_mode_display text too long ({} bytes), truncating to {} bytes",
                    encoded_text.len(),
                    Self::MAX_TEXT_BYTES
                );
                self.warned_text_truncated = true;
            }
        } else {
            self.warned_text_truncated = false;
        }

        let mut user_data = Vec::with_capacity(text_len + 1);
        user_data.push(text_coding_scheme);
        user_data.extend_from_slice(&encoded_text[..text_len]);

        let payload = Self::build_sds_tl_transfer_payload(protocol_id, self.msg_ref, &user_data);
        self.msg_ref = self.msg_ref.wrapping_add(1);
        self.last_tx = Some(dltime);

        Some(HomeModeDisplayTx {
            source_issi,
            dest_gssi: Self::HOME_MODE_BROADCAST_GSSI,
            payload,
        })
    }

    fn reset_timers(&mut self) {
        self.start_time = None;
        self.last_tx = None;
    }

    fn encode_text(text: &str, text_coding_scheme: u8) -> (Vec<u8>, bool) {
        match text_coding_scheme {
            // 0x1A = ISO/IEC 10646-1 UCS-2/UTF-16BE (ETSI EN 300 392-2 table 29.29)
            0x1A => (
                text.encode_utf16().flat_map(|u| [(u >> 8) as u8, (u & 0xFF) as u8]).collect(),
                false,
            ),
            // 0x01 = ISO/IEC 8859-1 Latin 1 (8-bit). Replace unsupported chars with '?'.
            0x01 => {
                let mut lossy = false;
                let mut out = Vec::with_capacity(text.len());
                for ch in text.chars() {
                    let cp = ch as u32;
                    if cp <= 0xFF {
                        out.push(cp as u8);
                    } else {
                        out.push(b'?');
                        lossy = true;
                    }
                }
                (out, lossy)
            }
            // Other schemes: pass raw UTF-8 bytes through unchanged.
            _ => (text.as_bytes().to_vec(), false),
        }
    }

    fn text_coding_scheme_to_sds_tl_value(scheme: HomeModeDisplaySdsTextCodingScheme) -> u8 {
        match scheme {
            HomeModeDisplaySdsTextCodingScheme::LATIN => 0x01,
            HomeModeDisplaySdsTextCodingScheme::UTF16 => 0x1A,
        }
    }

    /// Build an SDS-TL TRANSFER payload per ETSI EN 300 392-2 clause 29.4.2.4.
    ///
    /// Layout: protocol_id(8) | message_type(4)=0 | delivery_report_req(2)=0 |
    ///         service_selection(1)=0 | storage_forward_control(1)=0 |
    ///         message_reference(8) | user_data(variable)
    fn build_sds_tl_transfer_payload(protocol_id: u8, message_reference: u8, user_data: &[u8]) -> SdsUserData {
        let mut payload = BitBuffer::new_autoexpand((3 + user_data.len()) * 8);
        payload.write_bits(protocol_id as u64, 8);
        payload.write_bits(0, 4); // Message type: SDS-TRANSFER
        payload.write_bits(0, 2); // Delivery report request: none
        payload.write_bits(0, 1); // Service selection/short form report: short form recommended
        payload.write_bits(0, 1); // Storage/forward control: not available
        payload.write_bits(message_reference as u64, 8);
        for b in user_data {
            payload.write_bits(*b as u64, 8);
        }

        let bits = payload.get_len_written() as u16;
        let mut bytes = payload.into_bytes();
        bytes.truncate((bits as usize + 7) / 8);
        SdsUserData::Type4(bits, bytes)
    }
}
