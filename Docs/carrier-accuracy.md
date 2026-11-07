# Carrier accuracy, `ppm_err` and Hytera terminals

A TETRA base station must keep its carrier within **0.2 ppm** of nominal (EN 300 392-2),
which is under 100 Hz at 438 MHz. Low-cost SDRs and especially clones (B210 clones, cheap
Lime boards) can be one or two kilohertz off. The consequences depend on the radio:

- **Sepura** terminals track a carrier a couple of kilohertz off and register as if nothing
  were wrong, so an off-frequency station can run for months without anyone noticing.
- **Hytera** terminals (PT580H, PT580H Plus and relatives) do not. They never find the cell:
  the display shows the crossed antenna, the station log shows no access burst at all, and
  nothing in the codeplug changes that.

Seen on a B210 clone in November 2026: carrier 1.56 kHz low, two Sepuras registered, a
PT580H Plus with a correct codeplug never transmitted. With the carrier corrected to within
50 Hz it registered seven seconds after the station restarted.

## Measuring

The station cannot measure its own transmitter, so use a receiver that is not the station:
an RTL-SDR on any computer within range (a few metres from the antenna, not on it) and
`tools/carrier-offset.py`:

```bash
sudo apt install rtl-sdr
tools/carrier-offset.py 438.025                 # the station's tx_freq in MHz
```

```
carrier     438.023439 MHz  (-1561 Hz from 438.025000)
channel     438.0118 … 438.0347 MHz  (22.9 kHz wide, 39 dB above the floor)
verdict     1561 Hz off: outside the ±88 Hz limit. Sepura radios may still register; Hytera radios will not.
set         [phy_io.soapysdr] ppm_err = 3.56   (currently 0), restart the station and measure again
```

The script sweeps 100 kHz around the nominal frequency in 100 Hz bins, finds the 25 kHz
TETRA channel and takes its power centroid as the carrier (TETRA has no pilot tone). If the
station already has a correction, pass it with `--ppm-now` so the suggested value builds on
it. `--csv file` analyses an `rtl_power` sweep made elsewhere.

The dongle's own error is included in the measurement. An RTL-SDR with a TCXO (the Blog V3
and V4) is within about ±1 ppm, so ±450 Hz at most at 438 MHz and usually far less; an older
dongle with a plain crystal can be ±50 ppm and must be calibrated first (for example with
`kal` against a GSM carrier, or against a known-good transmitter), then passed as `--rtl-ppm`.

## Correcting

`ppm_err` under `[phy_io.soapysdr]` (dashboard: Configuration → Hardware RF → PPM) scales
every tuned frequency by `1 + ppm_err / 1e6`, downlink and uplink alike. A station that
transmits low needs a positive value: 1.56 kHz low at 438.025 MHz is 3.56 ppm. Set it,
restart, measure again; aim for under 100 Hz. Then check the radios still register: the
uplink moves by the same amount, so a terminal that was only just inside the station's
receive window may now be squarely in it.

The correction is per SDR, not per station: if you swap the board, measure again. Oscillator
drift with temperature is usually well under the limit once the board has warmed up, but a
board that was corrected cold may read a few hundred hertz off after an hour; measure warm.

## Not this

- Colour code, MCC/MNC and subscriber class do not need to match "specially" for Hytera; the
  usual values work. A Hytera that sees no cell at all is a carrier problem (or a frequency
  list problem on the radio) until proved otherwise.
- `freq_offset` in `[cell_info]` is the TETRA ±6.25/12.5 kHz channel offset broadcast to the
  radios. It is not a correction and must not be used as one.
