```
░█▀▀░█░░░█▀█░█░░░█░█░█▀▀░▀█▀░█▀█░▀█▀░▀█▀░█▀█░█▀█
░█▀▀░█░░░█░█░█░█░█░█░▀▀█░░█░░█▀█░░█░░░█░░█░█░█░█
░▀░░░▀▀▀░▀▀▀░▀▀▀▀░▀▀▀░▀▀▀░░▀░░▀░▀░░▀░░▀▀▀░▀▀▀░▀░▀
```

**FlowStation** este un fork al proiectului [tetra-bluestation](https://github.com/MidnightBlueLabs/tetra-bluestation) (MidnightBlueLabs), cu îmbunătățiri de stabilitate, corectarea unor bug-uri critice și funcționalități extinse.

---

## Ramuri

| Ramură | Scop |
|--------|------|
| `main` | Versiuni stabile, testate |
| `beta` | Versiuni în lucru, funcționalități noi |

---

## Îmbunătățiri față de upstream

### Stabilitate
- **Fix major ExpiryOfTimer loop** — `release_group_call` notifică acum Brew cu `NetworkCallEnd` la expirarea unui apel de grup inițiat din rețea. Fără acest fix, Brew rămânea cu apelul activ în starea sa și continua să trimită `NetworkCallStart` cu noi vorbitori, generând un loop de mii de apeluri expirate cu `ExpiryOfTimer`.
- **Reducere zgomot în log-uri** — warning-urile false (`setting expected ack for ts1`, `brew_uuid changed during speaker change`) degradate la nivel `trace`/`debug`, deoarece reprezintă comportament normal, nu erori reale.

### Apeluri individuale simplex (half-duplex P2P)
- **`transmission_request_permission`** setat corect la `false` (= 0 = permis) în `D-CONNECT`, `D-CONNECT-ACK`, `D-TX-CEASED` și `D-TX-GRANTED` — fix pentru "Not allowed to transmit" pe radiourile Motorola/Sepura.
- **Floor grant explicit la eliberare PTT** — la primirea `U-TX-CEASED`, BS trimite `D-TX-CEASED` vorbitorului și `D-TX-GRANTED(Granted)` peer-ului, în loc de `D-TX-CEASED` la ambii. Radiourile cu `GrantedToOtherUser` în `D-CONNECT` necesită un `D-TX-GRANTED` explicit pentru a activa butonul PTT.

---

## Instalare

Descarcă arhiva din [Releases](../../releases), dezarhivează și urmează instrucțiunile din documentație:

```bash
tar -xzf flowstation-*.tar.gz
cd tetra-bluestation
cp example_config/config.toml ./config.toml
# editează config.toml pentru parametrii tăi
cargo build --release
```

> **Notă:** folderul dezarhivat este `tetra-bluestation/` pentru compatibilitate cu documentația și scripturile existente.

---

## Configurare

Copiază fișierul de configurare exemplu și editează-l:

```bash
cp example_config/config.toml ./config.toml
```

Parametrii noi față de upstream:

| Parametru | Implicit | Descriere |
|-----------|----------|-----------|
| `hangtime_secs` | `5` | Durata menținerii circuitului unui apel de grup după eliberarea floor-ului (secunde) |
| `call_timeout_secs` | `120` | Durata maximă a unui apel activ înainte de D-RELEASE forțat (secunde) |
| `ul_inactivity_secs` | `3` | Timeout inactivitate UL după care BS forțează TX-CEASED (secunde) |

---

## Compilare

Cerințe: **Rust** (ultima versiune stabilă), **SoapySDR** cu driverele pentru SDR-ul tău.

```bash
cargo build --release
```

Binarul generat: `target/release/bluestation-bs`

---

## Documentație

Documentația de bază (hardware, configurare, build) este menținută de upstream:

[https://github.com/MidnightBlueLabs/tetra-bluestation-docs/wiki](https://github.com/MidnightBlueLabs/tetra-bluestation-docs/wiki)

---

## Mulțumiri

- **Harald Welte** și echipa **osmocom** pentru munca inițială pe osmocom-tetra, fără de care acest proiect nu ar fi existat.
- **Tatu Peltola**, care a extins rust-soapysdr cu funcționalitatea de timestamping necesară pentru rx/tx robust, și a furnizat un encoder/decoder Viterbi nativ Rust folosit în LMAC.
- Echipei **MidnightBlueLabs** pentru tetra-bluestation, baza pe care este construit FlowStation.
- **Stichting NLnet**, care a alocat o parte din grantu [proiectului RETETRA3](https://nlnet.nl/project/RETETRA3/) pentru implementarea de software FOSS pentru TETRA.

---

## Licență

Apache 2.0 — vezi [LICENSE](LICENSE)
