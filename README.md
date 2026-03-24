# zigbee-rs

A complete Zigbee PRO R22 protocol stack written in Rust, targeting embedded
`no_std` environments. Built on `async` traits for seamless integration with
Embassy and other embedded async runtimes.

```text
22,400+ lines of Rust · 108 source files · 9 crates · 33 ZCL clusters
```

## Architecture

```text
┌──────────────────────────────────────────────────────┐
│                    zigbee (top)                       │
│           coordinator · router · re-exports           │
├──────────────────────────────────────────────────────┤
│  zigbee-runtime   │  zigbee-bdb    │  zigbee-zcl     │
│  builder, power,  │  commissioning │  33 clusters,    │
│  NV storage,      │  steering,     │  foundation,     │
│  device templates  │  formation     │  reporting       │
├───────────────────┴────────────────┴─────────────────┤
│                    zigbee-zdo                          │
│          discovery · binding · network mgmt           │
├──────────────────────────────────────────────────────┤
│                    zigbee-aps                          │
│          frames · binding · groups · security         │
├──────────────────────────────────────────────────────┤
│                    zigbee-nwk                          │
│      frames · routing (AODV+tree) · security · NIB   │
├──────────────────────────────────────────────────────┤
│                    zigbee-mac                          │
│    MacDriver trait · MockMac · ESP32 · nRF52840      │
├──────────────────────────────────────────────────────┤
│                   zigbee-types                         │
│     IeeeAddress · ShortAddress · PanId · Channel     │
└──────────────────────────────────────────────────────┘
```

## Quick Start

### Mock examples (no hardware needed)

```bash
# Temperature + humidity sensor simulation
cargo run -p mock-sensor

# Coordinator (network formation + device join)
cargo run -p mock-coordinator

# Dimmable light
cargo run -p mock-light

# Sleepy end device (full SED lifecycle)
cargo run -p mock-sleepy-sensor
```

### Build the entire workspace

```bash
cargo build
cargo test    # (tests in progress)
```

### ESP32-C6 / ESP32-H2 firmware

```bash
cd examples/esp32c6-sensor
cargo build --release
espflash flash target/riscv32imac-unknown-none-elf/release/esp32c6-sensor
```

### nRF52840 firmware

```bash
cd examples/nrf52840-sensor
cargo build --release
probe-rs run --chip nRF52840_xxAA target/thumbv7em-none-eabihf/release/nrf52840-sensor
```

## MAC Backends

| Backend | Status | Target |
|---------|--------|--------|
| **MockMac** | ✅ Complete | Host (macOS/Linux/Windows) |
| **ESP32-C6/H2/C5** | ✅ Complete | `riscv32imac-unknown-none-elf` |
| **nRF52840** | ✅ Complete | `thumbv7em-none-eabihf` |
| STM32WB55 | 🔲 Skeleton | `thumbv7em-none-eabihf` |
| EFR32MG24 | 🔲 Skeleton | `thumbv7em-none-eabihf` |
| CC2652 | 🔲 Skeleton | `thumbv7em-none-eabihf` |

## ZCL Clusters (33)

**General:** Basic, Power Config, Identify, Groups, Scenes, On/Off, On/Off Switch Config,
Level Control, Alarms, Time, Multistate Input, OTA Upgrade, Poll Control, Green Power,
Diagnostics

**Closures:** Door Lock, Window Covering

**HVAC:** Thermostat, Fan Control, Thermostat UI Config

**Lighting:** Color Control

**Measurement:** Illuminance, Temperature, Pressure, Flow, Humidity, Occupancy, Electrical

**Security:** IAS Zone, IAS ACE, IAS WD

**Smart Energy:** Metering

**Touchlink:** Commissioning

## Design Principles

- **`#![no_std]`** everywhere — no heap allocation, `heapless` for bounded collections
- **`async` MacDriver trait** — 13 methods, no `Send`/`Sync` requirement
- **Platform-agnostic** — same stack code runs on mock, ESP32, nRF52840
- **Manual frame parsing** — no `serde`, bitfield encode/decode for all frame types
- **Embassy-compatible** — designed for single-threaded async executors
- **Layered crates** — each layer wraps the one below: `NwkLayer<M: MacDriver>`

## Project Structure

```
zigbee-rs-fork/
├── zigbee-types/          # Core types (addresses, channels)
├── zigbee-mac/            # MAC layer + platform backends
│   └── src/mock/          # Full mock for host testing
├── zigbee-nwk/            # Network layer (routing, security)
├── zigbee-aps/            # Application Support (binding, groups)
├── zigbee-zdo/            # Device Objects (discovery, mgmt)
├── zigbee-bdb/            # Base Device Behavior (commissioning)
├── zigbee-zcl/            # Zigbee Cluster Library (33 clusters)
├── zigbee-runtime/        # Device builder, power, NV storage
├── zigbee/                # Top-level: coordinator, router
├── tests/                 # Integration tests
├── examples/
│   ├── mock-sensor/       # Host: temp+humidity sensor
│   ├── mock-coordinator/  # Host: coordinator
│   ├── mock-light/        # Host: dimmable light
│   ├── mock-sleepy-sensor/# Host: SED demo
│   ├── esp32c6-sensor/    # ESP32-C6 + SHT31
│   └── nrf52840-sensor/   # nRF52840 + BME280
└── BUILD.md               # Comprehensive build guide
```

## Known Limitations

- **AES-CCM\* encryption** is real (using RustCrypto `aes` + `ccm` crates, `no_std`)
- **STM32WB55 / EFR32MG24 / CC2652** backends are skeletons (waiting for Rust ecosystem maturity)
- **No USB serial MAC** — can't bridge host ↔ dongle for real RF from desktop (yet)
- **Test coverage** is basic — the 4 mock examples exercise more than the test crate

## Documentation

See [BUILD.md](BUILD.md) for detailed instructions on building, flashing, sensor/display
integration, debugging, and peripheral wiring.

## License

GPL-2.0 (forked from zigbee-rs)
