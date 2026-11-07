#!/usr/bin/env python3
"""Measure how far a Bost FlowStation carrier sits from its nominal frequency
and print the `ppm_err` that centres it.

Why: the station cannot measure its own transmitter. Low-cost SDR clones can be
one or two kilohertz off at 400 MHz, which Sepura terminals forgive and Hytera
terminals do not (they never find the cell). The ETSI limit for a base station is
0.2 ppm, under 100 Hz at 438 MHz.

Needs an RTL-SDR (or any dongle `rtl_power` drives) within range of the station.

    tools/carrier-offset.py 438.025                  # sweep and report
    tools/carrier-offset.py 438.025 --ppm-now 3.4    # station already has ppm_err = 3.4
    tools/carrier-offset.py 438.025 --rtl-ppm 1      # the dongle's own correction
    tools/carrier-offset.py 438.025 --csv sweep.csv  # analyse an rtl_power CSV instead

The TETRA channel is 25 kHz wide with no pilot, so the carrier is taken as the
power centroid of the channel's energy; the edges are reported too so you can
see the shape. Measure with the dongle a few metres from the antenna, not on it.
"""

import argparse
import csv
import math
import shutil
import subprocess
import sys
import tempfile

SPAN_HZ = 100_000      # swept width around the nominal frequency
BIN_HZ = 100           # rtl_power bin size
CHANNEL_HZ = 25_000    # TETRA channel width


def sweep(freq_hz, rtl_ppm, gain, seconds, path):
    if shutil.which("rtl_power") is None:
        sys.exit("rtl_power not found: install rtl-sdr (apt install rtl-sdr) or pass --csv")
    lo, hi = freq_hz - SPAN_HZ // 2, freq_hz + SPAN_HZ // 2
    cmd = ["rtl_power", "-f", f"{lo}:{hi}:{BIN_HZ}", "-g", str(gain), "-i", str(seconds),
           "-1", "-e", f"{seconds}s"]
    if rtl_ppm:
        cmd += ["-p", str(rtl_ppm)]
    cmd.append(path)
    r = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    if r.returncode not in (0, 1) and "exiting" not in r.stderr:   # -1 exits via "User cancel"
        sys.exit(f"rtl_power failed:\n{r.stderr}")


def load_bins(path):
    bins = []
    with open(path, newline="") as f:
        for row in csv.reader(f):
            if len(row) < 7:
                continue
            lo, step = float(row[2]), float(row[4])
            for i, v in enumerate(row[6:]):
                try:
                    bins.append((lo + i * step, float(v)))
                except ValueError:
                    pass
    bins.sort()
    return bins


def analyse(bins, freq_hz):
    if len(bins) < 50:
        sys.exit("not enough bins in the sweep")
    levels = sorted(v for _, v in bins)
    noise = levels[len(levels) // 4]
    near = [(f, v) for f, v in bins if abs(f - freq_hz) <= CHANNEL_HZ]
    peak = max(v for _, v in near)
    if peak - noise < 12:
        sys.exit(f"no carrier found near {freq_hz / 1e6:.6f} MHz (peak only {peak - noise:.1f} dB above the floor)")
    # The channel is the contiguous run of bins well above the floor around the strongest one.
    thresh = noise + max(12.0, (peak - noise) * 0.45)
    on = [f for f, v in near if v >= thresh]
    lo, hi = min(on), max(on)
    win = [(f, v) for f, v in bins if lo <= f <= hi]
    weights = [10 ** (v / 10) for _, v in win]
    centroid = sum(f * w for (f, _), w in zip(win, weights)) / sum(weights)
    return {
        "noise_db": noise, "snr_db": peak - noise,
        "lo": lo, "hi": hi, "width": hi - lo,
        "centroid": centroid, "offset": centroid - freq_hz,
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("freq_mhz", type=float, help="nominal downlink frequency in MHz (the station's tx_freq)")
    ap.add_argument("--ppm-now", type=float, default=0.0, help="ppm_err currently set on the station (default 0)")
    ap.add_argument("--rtl-ppm", type=float, default=0.0, help="the dongle's own ppm correction, if you know it")
    ap.add_argument("--gain", type=float, default=20.0, help="dongle gain in dB (default 20; lower it if the carrier saturates)")
    ap.add_argument("--seconds", type=int, default=10, help="integration time (default 10)")
    ap.add_argument("--csv", help="analyse an existing rtl_power CSV instead of sweeping")
    args = ap.parse_args()

    freq_hz = round(args.freq_mhz * 1e6)
    path = args.csv
    if path is None:
        tmp = tempfile.NamedTemporaryFile(prefix="carrier-", suffix=".csv", delete=False)
        tmp.close()
        path = tmp.name
        print(f"sweeping {args.freq_mhz:.6f} MHz for {args.seconds} s …", file=sys.stderr)
        sweep(freq_hz, args.rtl_ppm, args.gain, args.seconds, path)

    r = analyse(load_bins(path), freq_hz)
    f_mhz = freq_hz / 1e6
    ppm_new = args.ppm_now - r["offset"] / f_mhz

    print(f"carrier     {r['centroid'] / 1e6:.6f} MHz  ({r['offset']:+.0f} Hz from {f_mhz:.6f})")
    print(f"channel     {r['lo'] / 1e6:.4f} … {r['hi'] / 1e6:.4f} MHz  ({r['width'] / 1e3:.1f} kHz wide, {r['snr_db']:.0f} dB above the floor)")
    if abs(r["width"] - CHANNEL_HZ) > 8_000:
        print("note        that is not the width of one TETRA channel; check the frequency and that only this cell is on air")
    tol = 0.2 * f_mhz   # ETSI base-station limit, in Hz
    if abs(r["offset"]) <= max(tol, 100):
        print(f"verdict     within the base-station limit (±{tol:.0f} Hz); no change needed")
    else:
        print(f"verdict     {abs(r['offset']):.0f} Hz off: outside the ±{tol:.0f} Hz limit. Sepura radios may still register; Hytera radios will not.")
        print(f"set         [phy_io.soapysdr] ppm_err = {ppm_new:.2f}   (currently {args.ppm_now:g}), restart the station and measure again")
    if not args.rtl_ppm:
        print("caveat      the dongle's own error is included; an uncalibrated RTL-SDR can add a few hundred hertz. Pass --rtl-ppm if you know it.")


if __name__ == "__main__":
    main()
