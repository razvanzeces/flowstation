//! Resampling, buffering and timestamp handling
//! between SDR device and modulator/demodulator code.

use rustfft;
use tetra_config::bluestation::SharedConfig;

use tetra_pdus::phy::traits::rxtx_dev::RxSlotBits;
use tetra_pdus::phy::traits::rxtx_dev::RxTxDev;
use tetra_pdus::phy::traits::rxtx_dev::RxTxDevError;
use tetra_pdus::phy::traits::rxtx_dev::TxSlotBits;

use crate::net_telemetry::channel::TelemetrySink;
use crate::net_telemetry::events::TelemetryEvent;
use crate::phy::components::soapy_dev;

use super::demodulator;
use super::dsp_types::*;
use super::fcfb;
use super::modulator;
use super::soapyio;

pub struct SdrConfig<'a> {
    /// SoapySDR device arguments
    pub dev_args: &'a [(&'a str, &'a str)],
    /// SDR RX center frequency
    pub rx_freq: Option<f64>,
    /// SDR TX center frequency
    pub tx_freq: Option<f64>,
}

#[derive(Default)]
pub struct PhyConfig<'a> {
    /// Downlink/uplink carrier frequency pairs to monitor.
    /// Uplink frequency can be set to None to monitor downlink only.
    pub monitor_frequencies: &'a [(f64, Option<f64>)],
    /// Carrier numbers corresponding to `monitor_frequencies`, if any.
    pub monitor_carrier_numbers: &'a [u16],
    /// Downlink carrier frequencies for a BS.
    pub bs_dl_frequencies: &'a [f64],
    /// Uplink carrier frequencies for a BS.
    pub bs_ul_frequencies: &'a [f64],
    /// Carrier numbers corresponding to BS uplink/downlink frequencies.
    pub bs_carrier_numbers: &'a [u16],
}

pub struct RxTxDevSoapySdr {
    sdr: soapyio::SoapyIo,
    rx_dsp: Option<RxDsp>,
    tx_dsp: Option<TxDsp>,
    health: Option<SdrHealthMonitor>,
}

type FftPlanner = rustfft::FftPlanner<RealSample>;

impl RxTxDevSoapySdr {
    pub fn new(cfg: &SharedConfig) -> Self {
        Self::with_telemetry(cfg, None)
    }

    /// Construct with an attached telemetry sink so the TX DSP can stream
    /// live spectrum + constellation snapshots to the dashboard.
    pub fn with_telemetry(cfg: &SharedConfig, telemetry: Option<TelemetrySink>) -> Self {
        let mut fft_planner = rustfft::FftPlanner::new();

        // TODO FIXME currently no MS and MON support in the below statement; need to fix
        let config_guard = cfg.config();
        let soapy_cfg = config_guard
            .as_ref()
            .phy_io
            .soapysdr
            .as_ref()
            .expect("Soapysdr config must be set for Soapysdr PhyIo");

        let (dl_corrected, dl_err) = soapy_cfg.dl_freq_corrected();
        let (ul_corrected, ul_err) = soapy_cfg.ul_freq_corrected();

        tracing::info!(
            "Freqs: DL / UL: {:.6} MHz / {:.6} MHz   PPM: {:.2} -> err {:.0} / {:.0} hz, adj {:.6} MHz / {:.6} MHz",
            soapy_cfg.dl_freq / 1e6,
            soapy_cfg.ul_freq / 1e6,
            soapy_cfg.ppm_err,
            dl_err,
            ul_err,
            dl_corrected / 1e6,
            ul_corrected / 1e6
        );

        let bs_carriers = config_guard
            .as_ref()
            .bs_phase_mod_carriers()
            .expect("validated carrier configuration should compute");
        let bs_carrier_numbers = bs_carriers.iter().map(|(carrier_num, _, _)| *carrier_num).collect::<Vec<_>>();
        let bs_dl_frequencies = bs_carriers.iter().map(|(_, dl_hz, _)| *dl_hz as f64).collect::<Vec<_>>();
        let bs_ul_frequencies = bs_carriers.iter().map(|(_, _, ul_hz)| *ul_hz as f64).collect::<Vec<_>>();

        let phy_config = soapy_dev::PhyConfig {
            monitor_frequencies: &[],
            monitor_carrier_numbers: &[],
            bs_dl_frequencies: &bs_dl_frequencies,
            bs_ul_frequencies: &bs_ul_frequencies,
            bs_carrier_numbers: &bs_carrier_numbers,
        };

        let mut sdr = match soapyio::SoapyIo::new(cfg) {
            Ok(sdr) => sdr,
            Err(e) => {
                // Failing to open the SDR at boot is fatal — there's nothing to transmit
                // on. A panic here is acceptable (and systemd will retry), but the default
                // .unwrap() message ("called Result::unwrap() on an Err...soapy_dev.rs:90")
                // tells the operator nothing. Surface the actual cause and the usual
                // culprits so log-only debugging is possible.
                tracing::error!(
                    "Failed to open SDR device: {}. Check that the SDR is plugged in, \
                     not held by another process (SoapySDRUtil/GQRX/another bluestation-bs), \
                     and that the device driver in [phy_io.soapysdr] matches the hardware. \
                     Cannot start without a radio.",
                    e
                );
                panic!("Failed to open SDR device: {e}");
            }
        };

        let health_telemetry = telemetry.clone();

        Self {
            rx_dsp: if sdr.rx_enabled() {
                Some(RxDsp::new(&mut fft_planner, &mut sdr, &phy_config))
            } else {
                None
            },

            tx_dsp: if sdr.tx_enabled() {
                Some(TxDsp::new(&mut fft_planner, &mut sdr, &phy_config, telemetry))
            } else {
                None
            },

            health: health_telemetry.map(SdrHealthMonitor::new),

            sdr,
        }
    }

    /// Process a block of received signal.
    /// Return true if processing can be continued,
    /// false if a slot has been demodulated and rxtx_timeslot should return.
    fn process_rx_block(&mut self) -> Result<bool, RxTxDevError> {
        if let Some(rx_dsp) = &mut self.rx_dsp {
            rx_dsp.process_block(&mut self.sdr)
        } else {
            Ok(false)
        }
    }

    /// Produce a block of transmit signal.
    /// Return true if processing can be continued,
    /// false if more data is needed
    /// or if it wants to wait before producing more.
    fn process_tx_block(&mut self, tx_slot: &[TxSlotBits]) -> Result<bool, RxTxDevError> {
        if let Some(tx_dsp) = &mut self.tx_dsp {
            if self.sdr.tx_possible() {
                tx_dsp.process_block(&mut self.sdr, self.rx_dsp.as_ref().map(|rx_dsp| rx_dsp.rx_block_count), tx_slot)
            } else {
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }
}

impl RxTxDev for RxTxDevSoapySdr {
    fn rxtx_timeslot<'a>(
        &'a mut self,
        tx_slot: &[TxSlotBits],
        // TODO multiple demodulators
    ) -> Result<Vec<Option<RxSlotBits<'a>>>, RxTxDevError> {
        // First generate as much TX signal as possible at the moment.
        while self.process_tx_block(tx_slot)? {}

        while self.process_rx_block()? {
            // Continue producing TX signal if possible.
            while self.process_tx_block(tx_slot)? {}
        }

        // SDR health: temperature readback, throttled to once every ~10 s.
        // Done here (not on a separate thread) so we can borrow &self.sdr safely
        // without locking. Only the temperature is read live now; gains are cached after
        // the first read because they never change and the USB readback was stalling the
        // PHY thread (FH-BUG-023). The single remaining sensor read is cheap and rare.
        if let Some(health) = &mut self.health {
            health.tick(&self.sdr);
        }

        if let Some(rx_dsp) = &mut self.rx_dsp {
            Ok(rx_dsp.take_slot_bits())
        } else {
            Ok(Default::default())
        }
    }
}

struct RxDsp {
    rx_fcfb: fcfb::AnalysisInputProcessor,

    rx_block_size: fcfb::InputBlockSize,
    rx_buffer: Vec<ComplexSample>,
    /// How much of rx_buffer has been filled
    rx_buffer_i: usize,
    rx_block_count: fcfb::BlockCount,

    monitors: Vec<MonitorDlUlPair>,
    ul_demodulators: Vec<DemodulatorChannel>,
}

impl RxDsp {
    fn new(fft_planner: &mut FftPlanner, sdr: &mut soapyio::SoapyIo, phy_config: &PhyConfig) -> Self {
        let sdr_sample_rate = sdr.rx_sample_rate();
        let rx_fcfb_params = fcfb::AnalysisInputParameters {
            // Use a bin spacing of 500 Hz.
            // This is a submultiple of the 72 kHz modem sample rate
            // and allows tuning in steps of 500 Hz.
            fft_size: (sdr_sample_rate / 500.0).round() as usize,
            center_frequency: sdr.rx_center_frequency().unwrap(),
            sample_rate: sdr_sample_rate,
            overlap: fcfb::Overlap::O1_4,
        };

        let fcfb = fcfb::AnalysisInputProcessor::new(fft_planner, rx_fcfb_params);
        let rx_block_size = fcfb.input_block_size();

        Self {
            rx_block_size,
            rx_buffer: vec![num::zero(); rx_block_size.overlap + rx_block_size.new],
            rx_buffer_i: 0,
            rx_fcfb: fcfb,
            rx_block_count: 0,

            monitors: phy_config
                .monitor_frequencies
                .iter()
                .enumerate()
                .map(|(i, (dl_freq, ul_freq))| MonitorDlUlPair {
                    dl: DemodulatorChannel::new(
                        fft_planner,
                        rx_fcfb_params,
                        *dl_freq,
                        demodulator::Mode::DlUnsynchronized,
                        phy_config.monitor_carrier_numbers.get(i).copied().unwrap_or(0),
                    ),
                    ul: ul_freq.as_ref().map(|ul_freq| {
                        DemodulatorChannel::new(
                            fft_planner,
                            rx_fcfb_params,
                            *ul_freq,
                            demodulator::Mode::Idle,
                            phy_config.monitor_carrier_numbers.get(i).copied().unwrap_or(0),
                        )
                    }),
                })
                .collect(),

            ul_demodulators: phy_config
                .bs_ul_frequencies
                .iter()
                .zip(phy_config.bs_carrier_numbers.iter().copied())
                .map(|(ul_freq, carrier_num)| {
                    DemodulatorChannel::new(fft_planner, rx_fcfb_params, *ul_freq, demodulator::Mode::Ul, carrier_num)
                })
                .collect(),
        }
    }

    fn process_block(&mut self, sdr: &mut soapyio::SoapyIo) -> Result<bool, RxTxDevError> {
        self.receive_block(sdr)?;

        let fcfb_result = self.rx_fcfb.process(&self.rx_buffer[..], self.rx_block_count);

        let mut continue_processing = true;

        for pair in self.monitors.iter_mut() {
            let continue_dl = pair.dl.process(fcfb_result, self.rx_block_count);
            if let Some(ul) = &mut pair.ul {
                ul.demodulator.sync_to_demodulator(&pair.dl.demodulator);
                continue_processing = ul.process(fcfb_result, self.rx_block_count) && continue_processing;
            } else {
                continue_processing = continue_dl && continue_processing;
            }
        }

        for demod in self.ul_demodulators.iter_mut() {
            continue_processing = demod.process(fcfb_result, self.rx_block_count) && continue_processing;
        }

        Ok(continue_processing)
    }

    fn receive_block(&mut self, sdr: &mut soapyio::SoapyIo) -> Result<(), RxTxDevError> {
        self.rx_block_count += 1;

        // Copy overlapping part from previous block to the beginning
        self.rx_buffer
            .copy_within(self.rx_block_size.new..self.rx_block_size.new + self.rx_block_size.overlap, 0);
        self.rx_buffer_i = self.rx_block_size.overlap;

        loop {
            let result = sdr.receive(&mut self.rx_buffer[self.rx_buffer_i..])?;

            let block_size = self.rx_block_size.new as SampleCount;
            let expected_count = self.rx_block_count as SampleCount * block_size + self.rx_buffer_i as SampleCount;
            let samples_lost = result.count - expected_count;
            if samples_lost != 0 {
                // Samples have been lost.
                // Mark RX buffer as empty and skip the right number of samples
                // to receive the next full processing block in the next iteration.

                // Expected sample count for the next read,
                // assuming no more samples are lost.
                let next_count = result.count + result.len as SampleCount;
                // div_euclid always rounds down (towards negative numbers),
                // so use it with negations to round up to the next block.
                let next_possible_block = -next_count.div_euclid(-block_size) + 1;
                let next_block_beginning = next_possible_block * block_size;

                let mut samples_to_skip = next_block_beginning - next_count;

                tracing::warn!(
                    "Lost {} samples, skipping {} more samples and {} processing blocks",
                    samples_lost,
                    samples_to_skip,
                    next_possible_block - self.rx_block_count
                );

                self.rx_block_count = next_possible_block;
                self.rx_buffer_i = 0;

                // Repeat reads until the correct number of samples has been skipped.
                while samples_to_skip > 0 {
                    let result = sdr.receive(&mut self.rx_buffer[0..samples_to_skip as usize])?;
                    samples_to_skip -= result.len as SampleCount;
                }
            } else {
                self.rx_buffer_i += result.len;
                if self.rx_buffer_i == self.rx_buffer.len() {
                    // tracing::trace!("Received processing block {} ({} samples in SDR buffer)",
                    //     self.rx_block_count,
                    //     // incorrect if time is not available but does not really matter
                    //     sdr.rx_current_count().unwrap_or(0) - (result.count + result.len as SampleCount - 1),
                    // );
                    return Ok(());
                }
            }
        }
    }

    fn take_slot_bits<'a>(&'a mut self) -> Vec<Option<RxSlotBits<'a>>> {
        // TODO: avoid dynamic allocation here?
        let mut slot_bits = Vec::with_capacity(2 * self.monitors.len() + self.ul_demodulators.len());

        for pair in self.monitors.iter_mut() {
            slot_bits.push(pair.dl.demodulator.take_demodulated_slot());
            slot_bits.push(if let Some(ul) = &mut pair.ul {
                ul.demodulator.take_demodulated_slot()
            } else {
                None
            });
        }

        for demod in self.ul_demodulators.iter_mut() {
            slot_bits.push(demod.demodulator.take_demodulated_slot());
        }

        slot_bits
    }
}

struct TxDsp {
    fcfb: fcfb::SynthesisOutputProcessor,
    block_count: fcfb::BlockCount,
    initial_time: i64,
    modulators: Vec<ModulatorChannel>,
    monitor: Option<TxSignalMonitor>,
    /// Scratch buffer reused for cloning the fcfb output when the TX monitor
    /// wants to inspect it. Kept on the struct so we don't allocate on every
    /// monitored block; capacity grows once to fcfb output_block_size and stays.
    tx_signal_scratch: Vec<ComplexSample>,
}

impl TxDsp {
    fn new(fft_planner: &mut FftPlanner, sdr: &mut soapyio::SoapyIo, phy_config: &PhyConfig, telemetry: Option<TelemetrySink>) -> Self {
        let sdr_sample_rate = sdr.tx_sample_rate();
        let fcfb_params = fcfb::SynthesisOutputParameters {
            ifft_size: (sdr_sample_rate / 500.0).round() as usize,
            center_frequency: sdr.tx_center_frequency().unwrap(),
            sample_rate: sdr_sample_rate,
            overlap: fcfb::Overlap::O1_4,
        };

        let fcfb = fcfb::SynthesisOutputProcessor::new(fft_planner, fcfb_params);

        let mut modulators = Vec::<ModulatorChannel>::new();
        for dl_freq in phy_config.bs_dl_frequencies {
            modulators.push(ModulatorChannel::new(fft_planner, fcfb_params, *dl_freq, modulator::Mode::Dl));
        }

        let carriers = phy_config
            .bs_carrier_numbers
            .iter()
            .copied()
            .zip(phy_config.bs_dl_frequencies.iter().copied())
            .collect::<Vec<_>>();
        let monitor = telemetry.map(|sink| {
            TxSignalMonitor::new(
                fft_planner,
                sink,
                sdr_sample_rate as RealSample,
                sdr.tx_center_frequency().unwrap(),
                carriers,
            )
        });

        Self {
            fcfb,
            block_count: 0,
            initial_time: 0, // TODO: get it from RX
            modulators,
            monitor,
            tx_signal_scratch: Vec::new(),
        }
    }

    fn process_block(
        &mut self,
        sdr: &mut soapyio::SoapyIo,
        latest_rx_block: Option<fcfb::BlockCount>,
        tx_slot: &[TxSlotBits],
    ) -> Result<bool, RxTxDevError> {
        let current_sample = sdr.tx_current_count()?;
        // Current time as block count
        let current_block = current_sample.div_euclid(self.fcfb.output_block_size() as SampleCount);

        let d = self.block_count - current_block;
        // Skip TX blocks in the past or in too near future
        let dmin = 2; // how many blocks in future minimum
        if d < dmin {
            let new_block_count = current_block + dmin;
            tracing::warn!(
                "Too late to produce TX block {}, skipping {} TX blocks",
                self.block_count,
                new_block_count - self.block_count
            );
            self.block_count = new_block_count;
        }
        // Limit how far into future TX blocks are generated
        let dmax = 60;
        if d > dmax {
            return Ok(false);
        }
        // Also limit how far from the latest RX block TX blocks are generated.
        // This prevents TX from ending up in an infinite loop
        // which does not give a chance for RX signal to get processed.
        //
        // This is not strictly necessary right now but might become useful
        // with different modulator operating modes in the future.
        //
        // Maybe the limit using hardware time above is redundant.
        if let Some(latest_rx_block) = latest_rx_block {
            let d_rx = self.block_count - latest_rx_block;
            if d_rx > dmax {
                return Ok(false);
            }
        }

        for (modulator, tx_slot) in self.modulators.iter_mut().zip(tx_slot) {
            if !modulator.process(&mut self.fcfb, self.block_count, tx_slot) {
                return Ok(false);
            }
        }

        // Cheap upfront check: if the TX monitor isn't due to emit yet, skip the
        // whole observation path including the Vec clone. Without this guard we
        // were allocating ~5 KB per TX block (≈600 blocks/sec at 600 kHz sample
        // rate) just to feed an observer that only fires every 200 ms — pure
        // waste, and on slower hosts (RPi 4) it was the difference between
        // keeping up with the TX timeline and hitting "Too late to produce TX
        // block N" warnings every few seconds. See FH-BUG (ES4TIX) for the
        // v0.2.2 regression report.
        let want_monitor = self.monitor.as_ref().map(|m| m.should_emit()).unwrap_or(false);

        // Compute sample count using the (still-current) block_count before we
        // bump it, otherwise sdr.transmit gets a count that's one block ahead.
        let block_count_now = self.block_count;

        // fcfb.process() is NOT idempotent — calling it twice on the same block
        // yields zeros the second time (it transitions Output → Clear). So we
        // call it exactly once. When the monitor is active, we copy into the
        // scratch buffer (reused across blocks, allocates once); otherwise we
        // hand the &[ComplexSample] straight to sdr.transmit.
        if want_monitor {
            self.tx_signal_scratch.clear();
            self.tx_signal_scratch.extend_from_slice(self.fcfb.process());
            if let Some(monitor) = self.monitor.as_mut() {
                monitor.observe(&self.tx_signal_scratch, tx_slot, self.block_count);
            }
        }

        // Increment block count before calling sdr.transmit with ?,
        // so we do not end up producing the same block again even if transmit fails.
        self.block_count += 1;

        if want_monitor {
            let sdr_sample_count = self.tx_signal_scratch.len() as SampleCount * block_count_now;
            sdr.transmit(&self.tx_signal_scratch, Some(sdr_sample_count))?;
        } else {
            let tx_signal = self.fcfb.process();
            let sdr_sample_count = tx_signal.len() as SampleCount * block_count_now;
            sdr.transmit(tx_signal, Some(sdr_sample_count))?;
        }

        // tracing::trace!("Produced transmit block {} ({} samples in future)",
        //     self.block_count - 1,
        //     sdr_sample_count - sdr.tx_current_count().unwrap_or(0),
        // );

        Ok(true)
    }
}

struct DemodulatorChannel {
    downconverter: fcfb::AnalysisOutputProcessor,
    demodulator: demodulator::Demodulator,
}

impl DemodulatorChannel {
    fn new(
        fft_planner: &mut FftPlanner,
        analysis_in_params: fcfb::AnalysisInputParameters,
        frequency: f64,
        mode: demodulator::Mode,
        carrier_num: u16,
    ) -> Self {
        Self {
            downconverter: fcfb::AnalysisOutputProcessor::new_with_frequency(
                fft_planner,
                analysis_in_params,
                demodulator::SAMPLE_RATE,
                frequency,
                Some(25000.0),
            ),
            demodulator: demodulator::Demodulator::new(mode, carrier_num),
        }
    }

    /// Return true if processing should be continued,
    /// false if a new demodulated slot is available.
    fn process(&mut self, fcfb_result: &fcfb::AnalysisIntermediateResult, block_count: fcfb::BlockCount) -> bool {
        let samples = self.downconverter.process(fcfb_result);
        for (i, sample) in samples.iter().enumerate() {
            // TODO: include delay of FCFB in sample count
            self.demodulator.sample(
                *sample,
                block_count as SampleCount * samples.len() as SampleCount + i as SampleCount,
            );
        }
        !self.demodulator.demodulated_slot_available()
    }
}

struct ModulatorChannel {
    upconverter: fcfb::SynthesisInputProcessor,
    modulator: modulator::Modulator,
    /// Buffer for modulated signal at modulator sample rate.
    buffer: fcfb::InputBuffer,
    /// How much of buffer is filled
    buffer_i: usize,
}

impl ModulatorChannel {
    fn new(
        fft_planner: &mut FftPlanner,
        synthesis_out_params: fcfb::SynthesisOutputParameters,
        frequency: f64,
        mode: modulator::Mode,
    ) -> Self {
        let upconverter = fcfb::SynthesisInputProcessor::new_with_frequency(
            fft_planner,
            synthesis_out_params,
            modulator::SAMPLE_RATE,
            frequency,
            Some(25000.0),
        );
        Self {
            buffer: upconverter.make_input_buffer(),
            buffer_i: 0,
            upconverter,
            modulator: modulator::Modulator::new(mode),
        }
    }

    fn process(&mut self, fcfb: &mut fcfb::SynthesisOutputProcessor, block_count: fcfb::BlockCount, tx_slot: &TxSlotBits) -> bool {
        let buf = self.buffer.buffer_in();
        while self.buffer_i < buf.len() {
            // TODO: include delay of FCFB in sample count
            match self.modulator.sample(
                block_count as SampleCount * buf.len() as SampleCount + self.buffer_i as SampleCount,
                tx_slot,
            ) {
                Ok(sample) => {
                    buf[self.buffer_i] = sample;
                    self.buffer_i += 1;
                }
                Err(modulator::Error::NeedMoreData) => {
                    return false;
                }
            }
        }
        fcfb.add(self.upconverter.process(self.buffer.buffer(), block_count));

        let _ = self.buffer.prepare_for_new_samples();
        self.buffer_i = 0;
        true
    }
}

struct MonitorDlUlPair {
    dl: DemodulatorChannel,
    ul: Option<DemodulatorChannel>,
}

// ── TX signal monitor ─────────────────────────────────────────────────────
//
// Snapshots the complex baseband samples that FlowStation generates BEFORE they
// reach the SDR. Works on any radio (LimeSDR, SXceiver, µCell, USRP, Pluto) because
// the analysis is purely internal — we look at our own DSP output, not at anything
// the radio reads back.
//
// Each snapshot emits one of two TelemetryEvent variants — TxVisual (fast,
// ~5 Hz, carries spectrum + IQ) or TxQuality (slow, ~1 Hz, carries derived
// metrics). Split this way so the dashboard can animate the graphics live
// while keeping the numeric readouts calm.
//
// Both paths share the FFT and constellation recovery, so when both are due
// in the same call they are computed once and emitted in two messages.

struct TxSignalMonitor {
    sink: TelemetrySink,
    sample_rate: RealSample,
    center_frequency: f64,
    carriers: Vec<(u16, f64)>,
    fft: std::sync::Arc<dyn rustfft::Fft<RealSample>>,
    fft_buffer: Vec<ComplexSample>,
    window: Vec<RealSample>,
    constellation_history: Vec<ComplexSample>,
    /// Wall-clock time of the next allowed *visual* emission (spectrum + IQ).
    /// Rate-limiting on block_count is brittle because block size varies with
    /// sample rate (at 600 kHz one block is ~1.5 ms, at 2 MHz it's ~0.45 ms).
    /// Using wall-clock time gives a stable emit rate regardless of SDR config.
    next_visual_emit: std::time::Instant,
    /// Wall-clock time of the next allowed *quality* emission (EVM, PAPR, etc).
    /// Slower than visual so the numeric readouts don't flicker.
    next_quality_emit: std::time::Instant,
    /// Visual cadence — fast enough for fluid animation but not so fast that
    /// the FFT cost dominates. ~200 ms = 5 Hz feels live.
    visual_interval: std::time::Duration,
    /// Quality cadence — slower so the dashboard numbers settle. 1 s is the
    /// natural human-readable rate; combined with the front-end smoothing window
    /// it ends up looking rock-stable.
    quality_interval: std::time::Duration,
}

impl TxSignalMonitor {
    const FFT_LEN: usize = 512;
    const CONSTELLATION_POINTS: usize = 192;
    const CONSTELLATION_ENCODE_SCALE: RealSample = 32767.0 / 1.5;

    fn new(
        fft_planner: &mut FftPlanner,
        sink: TelemetrySink,
        sample_rate: RealSample,
        center_frequency: f64,
        carriers: Vec<(u16, f64)>,
    ) -> Self {
        let fft = fft_planner.plan_fft_forward(Self::FFT_LEN);
        let window = (0..Self::FFT_LEN)
            .map(|i| {
                let phase = 2.0 * std::f32::consts::PI * i as RealSample / (Self::FFT_LEN - 1) as RealSample;
                0.5 - 0.5 * phase.cos()
            })
            .collect();
        Self {
            sink,
            sample_rate,
            center_frequency,
            carriers,
            fft,
            fft_buffer: vec![ComplexSample::ZERO; Self::FFT_LEN],
            window,
            constellation_history: Vec::with_capacity(Self::CONSTELLATION_POINTS),
            next_visual_emit: std::time::Instant::now(),
            next_quality_emit: std::time::Instant::now(),
            visual_interval: std::time::Duration::from_millis(200),
            quality_interval: std::time::Duration::from_millis(1000),
        }
    }

    /// Cheap predicate: would a call to `observe()` actually emit anything
    /// right now? Used by the hot TX path to skip an expensive Vec clone when
    /// no telemetry is due.
    fn should_emit(&self) -> bool {
        let now = std::time::Instant::now();
        now >= self.next_visual_emit || now >= self.next_quality_emit
    }

    fn observe(&mut self, samples: &[ComplexSample], _tx_slots: &[TxSlotBits], _block_count: fcfb::BlockCount) {
        let now = std::time::Instant::now();
        let need_visual = now >= self.next_visual_emit;
        let need_quality = now >= self.next_quality_emit;
        if (!need_visual && !need_quality) || samples.len() < Self::FFT_LEN {
            return;
        }

        // ── Power statistics (cheap; needed for both visual & quality) ────
        // Visual needs RMS/peak for the topbar; Quality also needs them
        // (PAPR = peak - rms) and a few derived sums.
        let mut peak2: RealSample = 0.0;
        let mut sum2: RealSample = 0.0;
        // Mean accumulators for DC offset & IQ-balance estimation (only used
        // by the quality path, but the inner loop is so tight that splitting
        // would cost more in code complexity than the few extra adds save).
        let mut sum_i: RealSample = 0.0;
        let mut sum_q: RealSample = 0.0;
        let mut sum_i2: RealSample = 0.0;
        let mut sum_q2: RealSample = 0.0;
        let mut sum_iq: RealSample = 0.0;
        let n = samples.len() as RealSample;
        for sample in samples {
            let p = sample.norm_sqr();
            peak2 = peak2.max(p);
            sum2 += p;
            if need_quality {
                sum_i += sample.re;
                sum_q += sample.im;
                sum_i2 += sample.re * sample.re;
                sum_q2 += sample.im * sample.im;
                sum_iq += sample.re * sample.im;
            }
        }
        let rms = (sum2 / n).sqrt();
        let rms_dbfs = 20.0 * rms.max(1.0e-12).log10();
        let peak_dbfs = 20.0 * peak2.sqrt().max(1.0e-12).log10();

        // ── FFT (needed for spectrum AND for carrier-leak/OBW) ────────────
        // Take the centre FFT_LEN samples (avoids the band-edges of overlap-add
        // fcfb output where amplitude is artificially reduced).
        let start = (samples.len() - Self::FFT_LEN) / 2;
        for i in 0..Self::FFT_LEN {
            self.fft_buffer[i] = samples[start + i] * self.window[i];
        }
        self.fft.process(&mut self.fft_buffer);

        // Linear magnitude² in unshifted order — reused below.
        let mut mag2 = vec![0.0_f32; Self::FFT_LEN];
        for i in 0..Self::FFT_LEN {
            let m = self.fft_buffer[i].norm() / Self::FFT_LEN as RealSample;
            mag2[i] = m * m;
        }

        // dB-tenths spectrum, fftshift'd (DC in middle, negative freqs on left).
        let spectrum_db_tenths: Vec<i16> = (0..Self::FFT_LEN)
            .map(|i| {
                let idx = (i + Self::FFT_LEN / 2) % Self::FFT_LEN;
                let m = mag2[idx].sqrt();
                (20.0 * m.max(1.0e-12).log10() * 10.0)
                    .round()
                    .clamp(i16::MIN as RealSample, i16::MAX as RealSample) as i16
            })
            .collect();

        // Constellation recovery is also needed for the visual path AND for EVM.
        // We compute it once and reuse.
        let (constellation_iq, evm_pct) = self.measured_constellation_with_evm(samples);

        // ── Fast visual emit ─────────────────────────────────────────────
        if need_visual {
            self.next_visual_emit = now + self.visual_interval;
            self.sink.send(TelemetryEvent::TxVisual {
                sample_rate: self.sample_rate,
                center_freq_hz: self.center_frequency,
                carriers: self.carriers.clone(),
                rms_dbfs,
                peak_dbfs,
                spectrum_db_tenths,
                constellation_iq,
            });
        }

        // ── Slow quality emit ────────────────────────────────────────────
        if need_quality {
            self.next_quality_emit = now + self.quality_interval;

            // PAPR
            let papr_db = peak_dbfs - rms_dbfs;

            // DC offset
            let dc_offset_i = sum_i / n;
            let dc_offset_q = sum_q / n;

            // IQ amplitude imbalance (Var(I) vs Var(Q) in dB).
            let var_i = (sum_i2 / n) - dc_offset_i * dc_offset_i;
            let var_q = (sum_q2 / n) - dc_offset_q * dc_offset_q;
            let iq_amplitude_imbalance_db = if var_i > 1.0e-12 && var_q > 1.0e-12 {
                10.0 * (var_i / var_q).log10()
            } else {
                0.0
            };

            // IQ phase imbalance via E[I·Q] normalized by sqrt(Var(I)·Var(Q)).
            let cov_iq = (sum_iq / n) - dc_offset_i * dc_offset_q;
            let phase_sin = if var_i > 1.0e-12 && var_q > 1.0e-12 {
                (cov_iq / (var_i * var_q).sqrt()).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let iq_phase_imbalance_deg = phase_sin.asin().to_degrees();

            // Carrier leakage: DC-bin power vs total.
            let dc_power = mag2[0];
            let total_power: RealSample = mag2.iter().sum::<RealSample>().max(1.0e-12);
            let carrier_leakage_db = 10.0 * (dc_power / total_power).max(1.0e-12).log10();

            // Occupied bandwidth (ETSI 99%).
            let occupied_bandwidth_hz = occupied_bandwidth(&mag2, self.sample_rate, 0.99);

            self.sink.send(TelemetryEvent::TxQuality {
                papr_db,
                evm_pct,
                dc_offset_i,
                dc_offset_q,
                iq_amplitude_imbalance_db,
                iq_phase_imbalance_deg,
                carrier_leakage_db,
                occupied_bandwidth_hz,
            });
        }
    }

    /// Recover symbol-rate IQ samples AND compute RMS-normalized EVM in one pass.
    /// Returns (interleaved I,Q,... as i16, EVM in %). EVM is computed only over the
    /// freshly-derotated symbols in this call, not over the rolling history.
    fn measured_constellation_with_evm(&mut self, samples: &[ComplexSample]) -> (Vec<i16>, f32) {
        // TETRA symbol rate is 18 kbaud (π/4-DQPSK). At our typical 600 kHz SDR
        // sample rate that's 33.3 samples/symbol.
        let samples_per_symbol = self.sample_rate / 18_000.0;
        if !samples_per_symbol.is_finite() || samples_per_symbol < 1.0 {
            return (Vec::new(), 0.0);
        }

        let Some((phase, rotation, gain)) = constellation_timing_rotation_gain(samples, samples_per_symbol) else {
            return (Vec::new(), 0.0);
        };

        let (sin_rot, cos_rot) = rotation.sin_cos();
        // Accumulators for this call's EVM computation. RMS-normalized EVM per 3GPP TS 36.104:
        //   EVM = sqrt( mean(|measured − ideal|²) / mean(|ideal|²) )
        // For π/4-DQPSK the ideal points sit on the unit circle, so mean(|ideal|²) = 1.
        let mut err_sum: RealSample = 0.0;
        let mut err_count: usize = 0;
        let mut sample_at = phase;
        while sample_at < samples.len() as RealSample {
            let idx = sample_at.round() as usize;
            if let Some(sample) = samples.get(idx) {
                let derotated = ComplexSample {
                    re: (sample.re * cos_rot + sample.im * sin_rot) / gain,
                    im: (sample.im * cos_rot - sample.re * sin_rot) / gain,
                };
                if derotated.norm() > 0.05 {
                    self.constellation_history.push(derotated);

                    // EVM: snap to nearest of 8 ideal π/4-DQPSK constellation points (unit circle)
                    // and accumulate squared error.
                    let angle = derotated.im.atan2(derotated.re).rem_euclid(std::f32::consts::TAU);
                    let ideal_angle = (angle / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
                    let ideal = ComplexSample {
                        re: ideal_angle.cos(),
                        im: ideal_angle.sin(),
                    };
                    let err = derotated - ideal;
                    err_sum += err.norm_sqr();
                    err_count += 1;
                }
            }
            sample_at += samples_per_symbol;
        }

        let evm_pct = if err_count >= 8 {
            // sqrt(mean squared error) * 100. Ideal-power normalization factor is 1
            // because ideal points are on the unit circle (|ideal|² = 1).
            (err_sum / err_count as RealSample).sqrt() * 100.0
        } else {
            0.0
        };

        if self.constellation_history.len() > Self::CONSTELLATION_POINTS {
            let excess = self.constellation_history.len() - Self::CONSTELLATION_POINTS;
            self.constellation_history.drain(0..excess);
        }

        let mut points = Vec::with_capacity(self.constellation_history.len() * 2);
        for sample in &self.constellation_history {
            points.push(
                (sample.re.clamp(-1.5, 1.5) * Self::CONSTELLATION_ENCODE_SCALE)
                    .round()
                    .clamp(i16::MIN as RealSample, i16::MAX as RealSample) as i16,
            );
            points.push(
                (sample.im.clamp(-1.5, 1.5) * Self::CONSTELLATION_ENCODE_SCALE)
                    .round()
                    .clamp(i16::MIN as RealSample, i16::MAX as RealSample) as i16,
            );
        }
        (points, evm_pct)
    }
}

/// Find the smallest contiguous band around the centre bin that contains the
/// requested fraction of total power. Returns the bandwidth in Hz.
///
/// Walks outward from the centre bin (DC after fftshift) by symmetric pairs,
/// integrating power until the threshold is crossed. This matches the ETSI
/// occupied-bandwidth definition (99% for OBW-99) commonly used for spectrum
/// mask compliance.
///
/// `mag2_unshifted` is the magnitude-squared spectrum in standard (non-fftshift'd)
/// FFT order — bin 0 = DC, bins 1..N/2 = positive freqs, bins N/2..N = negative.
fn occupied_bandwidth(mag2_unshifted: &[RealSample], sample_rate: RealSample, fraction: RealSample) -> RealSample {
    let n = mag2_unshifted.len();
    if n < 4 {
        return 0.0;
    }
    let total: RealSample = mag2_unshifted.iter().sum::<RealSample>();
    if total <= 1.0e-12 {
        return 0.0;
    }
    let target = total * fraction;

    // Accumulate DC bin first, then symmetric pairs (k, N-k) representing +/- k.
    let mut accum = mag2_unshifted[0];
    let half = n / 2;
    for k in 1..=half {
        if accum >= target {
            // (k-1) bins each side of DC produced the threshold crossing.
            // Bandwidth covered = (2*(k-1)+1) bins → in Hz scaled by bin spacing.
            let bins = (2 * (k - 1) + 1) as RealSample;
            return bins * sample_rate / n as RealSample;
        }
        accum += mag2_unshifted[k];
        if k != half {
            accum += mag2_unshifted[n - k];
        }
    }
    // Whole band crosses → return full sample rate as the OBW.
    sample_rate
}

/// Sweep 64 timing offsets within one symbol period to find the best constellation
/// sampling instant. For each candidate offset, we estimate the constellation rotation
/// and gain, then score by squared distance to ideal π/4-DQPSK points. Lower score = better.
fn constellation_timing_rotation_gain(
    samples: &[ComplexSample],
    samples_per_symbol: RealSample,
) -> Option<(RealSample, RealSample, RealSample)> {
    const STEPS: usize = 64;
    let mut best: Option<(RealSample, RealSample, RealSample, RealSample)> = None;

    for step in 0..STEPS {
        let phase = samples_per_symbol * step as RealSample / STEPS as RealSample;
        let points = constellation_points_for_phase(samples, samples_per_symbol, phase);
        if points.len() < 8 {
            continue;
        }
        let rotation = constellation_rotation(&points)?;
        let (sin_rot, cos_rot) = rotation.sin_cos();
        let mut radius_sum = 0.0;
        let mut radius_count = 0usize;
        for point in &points {
            let derotated = ComplexSample {
                re: point.re * cos_rot + point.im * sin_rot,
                im: point.im * cos_rot - point.re * sin_rot,
            };
            let radius = derotated.norm();
            if radius > 1.0e-5 {
                radius_sum += radius;
                radius_count += 1;
            }
        }
        if radius_count < 8 {
            continue;
        }
        let gain = radius_sum / radius_count as RealSample;
        let mut err_sum = 0.0;
        let mut err_count = 0usize;
        for point in &points {
            let derotated = ComplexSample {
                re: (point.re * cos_rot + point.im * sin_rot) / gain,
                im: (point.im * cos_rot - point.re * sin_rot) / gain,
            };
            let radius = derotated.norm();
            if radius < 0.05 {
                continue;
            }
            let angle = derotated.im.atan2(derotated.re).rem_euclid(std::f32::consts::TAU);
            let ideal = (angle / (std::f32::consts::FRAC_PI_4)).round() * std::f32::consts::FRAC_PI_4;
            let ideal_point = ComplexSample {
                re: ideal.cos(),
                im: ideal.sin(),
            };
            let err = derotated - ideal_point;
            err_sum += err.norm_sqr();
            err_count += 1;
        }
        if err_count < 8 {
            continue;
        }
        let score = err_sum / err_count as RealSample;
        match best {
            Some((best_score, _, _, _)) if score >= best_score => {}
            _ => best = Some((score, phase, rotation, gain.max(1.0e-5))),
        }
    }

    best.map(|(_, phase, rotation, gain)| (phase, rotation, gain))
}

fn constellation_points_for_phase(samples: &[ComplexSample], samples_per_symbol: RealSample, phase: RealSample) -> Vec<ComplexSample> {
    let mut points = Vec::new();
    let mut sample_at = phase;
    while sample_at < samples.len() as RealSample {
        let idx = sample_at.round() as usize;
        if let Some(sample) = samples.get(idx) {
            points.push(*sample);
        }
        sample_at += samples_per_symbol;
    }
    points
}

/// Estimate the constellation rotation by treating each point as an 8th-order vector
/// (π/4-DQPSK has 8 ideal phases) and finding the dominant angle. Returns the rotation
/// in radians (the angle that, applied as a derotation, lines points up with the
/// real and imaginary axes).
fn constellation_rotation(points: &[ComplexSample]) -> Option<RealSample> {
    let max_radius = points.iter().map(|point| point.norm()).fold(0.0, RealSample::max);
    if max_radius <= 1.0e-6 {
        return None;
    }

    let min_radius = max_radius * 0.25;
    let mut sum_re = 0.0;
    let mut sum_im = 0.0;
    let mut weight_sum = 0.0;
    for point in points {
        let radius = point.norm();
        if radius < min_radius {
            continue;
        }
        let phase = point.im.atan2(point.re) * 8.0;
        let weight = radius * radius;
        sum_re += phase.cos() * weight;
        sum_im += phase.sin() * weight;
        weight_sum += weight;
    }

    if weight_sum <= 1.0e-9 {
        None
    } else {
        Some(sum_im.atan2(sum_re) / 8.0)
    }
}

// ── SDR hardware health monitor ──────────────────────────────────────────────
//
// Polls the radio every ~5 seconds for temperature and gain readback. Designed
// to be cheap enough to run inline on the PHY tick loop — Soapy reads are typically
// sub-millisecond. Emitted as TelemetryEvent::SdrHealth so the dashboard can show
// hardware temp, real gain values, and warn the operator about thermal drift.
//
// Implemented as a separate type (not bundled into TxSignalMonitor) because:
//   1. Different cadence (5 s vs 1 s).
//   2. Different data (no DSP, just hardware introspection).
//   3. Some SDRs don't expose any sensors — we still want TX DSP metrics on those.

struct SdrHealthMonitor {
    sink: TelemetrySink,
    /// Wall-clock time of next emission. Initialised to "now-ish" so the first tick fires immediately.
    next_emit: std::time::Instant,
    /// Polling interval.
    interval: std::time::Duration,
    /// Cached TX/RX gain readback. The gains are configured once at startup and never
    /// change at runtime, so reading them back from the SDR on every health tick is
    /// pointless — and on USB SDRs (LimeSDR Mini) each list_gains()/gain_element() call
    /// is a synchronous USB transaction taking 5-15 ms. Doing ~9 of them every 5 s on the
    /// PHY thread stalled TX block production for 45-135 ms, producing the "Too late to
    /// produce TX block, skipping N blocks" warnings every 5 s (FH-BUG-023). We now read
    /// the gains exactly once (lazily, on the first tick) and reuse the cached values.
    cached_tx_gains: Option<Vec<(String, f32)>>,
    cached_rx_gains: Option<Vec<(String, f32)>>,
}

impl SdrHealthMonitor {
    fn new(sink: TelemetrySink) -> Self {
        Self {
            sink,
            next_emit: std::time::Instant::now(),
            // 10 s rather than 5 s: temperature drifts slowly, and this halves the
            // residual PHY-thread cost of the one remaining sensor read.
            interval: std::time::Duration::from_secs(10),
            cached_tx_gains: None,
            cached_rx_gains: None,
        }
    }

    fn tick(&mut self, sdr: &soapyio::SoapyIo) {
        let now = std::time::Instant::now();
        if now < self.next_emit {
            return;
        }
        self.next_emit = now + self.interval;

        // Only the temperature is read live — it's a single sensor read and the value
        // genuinely changes. Gains are cached after the first read (see field docs).
        let temperature_c = sdr.read_temperature_c();

        if self.cached_tx_gains.is_none() {
            self.cached_tx_gains = Some(sdr.read_tx_gains());
        }
        if self.cached_rx_gains.is_none() {
            self.cached_rx_gains = Some(sdr.read_rx_gains());
        }
        let tx_gains = self.cached_tx_gains.clone().unwrap_or_default();
        let rx_gains = self.cached_rx_gains.clone().unwrap_or_default();

        self.sink.send(TelemetryEvent::SdrHealth {
            temperature_c,
            tx_gains,
            rx_gains,
        });
    }
}
