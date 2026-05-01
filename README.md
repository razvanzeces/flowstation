```
░█▀▀░█░░░█▀█░█░█░█▀▀░▀█▀░█▀█░▀█▀░▀█▀░█▀█░█▀█
░█▀▀░█░░░█░█░█▄█░▀▀█░░█░░█▀█░░█░░░█░░█░█░█░█
░▀░░░▀▀▀░▀▀▀░▀░▀░▀▀▀░░▀░░▀░▀░░▀░░▀▀▀░▀▀▀░▀░▀
```

**FlowStation** is a FOSS TETRA base station stack focused on stability, full-duplex voice support, and production-ready operation. It is a fork of [tetra-bluestation](https://github.com/MidnightBlueLabs/tetra-bluestation) by Wouter Bokslag / Midnight Blue, extended with:

- ✅ Full-duplex individual (P2P) calls — ETSI EN 300 392-2 §14 compliant
- ✅ Individual calls over Brew/TetraPack (circuit-switched, duplex)
- ✅ DTMF forwarding (U-INFO) to TetraPack
- ✅ Configurable hangtime, call timeout (T310), and UL inactivity timeout
- ✅ Graceful FACCH steal guard — no voice on closed circuits
- ✅ Jitter buffer flush on speaker change — clean audio transitions
- ✅ Stable reconnect handling — all active calls released on backhaul loss

## Changelog vs upstream

See `CHANGELOG.md` for a full list of changes relative to tetra-bluestation.

## Documentation

Hardware, configuration, and build instructions follow the upstream documentation:

https://github.com/MidnightBlueLabs/tetra-bluestation-docs/wiki

Configuration parameters added by FlowStation are documented in `example_config/config.toml`.

## License

AGPL-3.0 — same as upstream. See `LICENSE`.

## Acknowledgements

- **Wouter Bokslag / Midnight Blue** — original tetra-bluestation implementation
- **Harald Welte and the osmocom crew** — foundational work on osmocom-tetra
- **Tatu Peltola** — rust-soapysdr timestamping and Viterbi encoder/decoder
- **Stichting NLnet** — [RETETRA3 project](https://nlnet.nl/project/RETETRA3/) grant support
