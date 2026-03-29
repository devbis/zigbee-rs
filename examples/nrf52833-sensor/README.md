# nRF52833 Zigbee Temperature Sensor

An async Embassy-based Zigbee 3.0 end device for the **Nordic nRF52833** that
reads real temperature from the on-chip TEMP peripheral and reports simulated
humidity. Uses `defmt` + RTT for logging.

## Hardware Requirements

- nRF52833-DK (PCA10100) or any nRF52833 board with a debug probe
- Button 1 (P0.11, active low) for join/leave control
- Debug probe (J-Link on-board for DK, or external probe-rs-compatible)

## Prerequisites

- Rust stable toolchain
- `probe-rs`: `cargo install probe-rs-tools`
- Target: `thumbv7em-none-eabihf` (configured in `.cargo/config.toml`)

No vendor libraries, SoftDevice, or binary blobs are needed — the project
drives the 802.15.4 radio directly via `embassy-nrf`.

## Build

```sh
cargo build --release
```

## Flash & Run

```sh
probe-rs run --chip nRF52833_xxAA target/thumbv7em-none-eabihf/release/nrf52833-sensor
```

Or use the configured runner:

```sh
cargo run --release
```

## What It Demonstrates

- Embassy async event loop with `select3` (radio receive, button press, timer)
- On-chip TEMP sensor reading via `embassy_nrf::temp::Temp`
- Building a Zigbee device with `ZigbeeDevice` builder API
- ZCL endpoint 1 (Home Automation, device type 0x0302) with
  **Temperature Measurement** and **Relative Humidity** server clusters
- Processing incoming MAC frames and generating ZCL attribute reports
- Button-driven network join/leave via `UserAction::Toggle`
- `defmt` structured logging over RTT

## Differences from nRF52840-sensor

- Uses `nrf52833` feature for `embassy-nrf` and `zigbee-mac`
- Chip: nRF52833_xxAA (512 KB Flash, 128 KB RAM vs 1 MB / 256 KB)
- Runner: `probe-rs run --chip nRF52833_xxAA`

## Operation

1. Power on → device starts idle (not joined)
2. Press Button 1 → initiates BDB commissioning (network steering)
3. Once joined → reads temperature every 30 s, ticks the Zigbee stack
4. Press Button 1 again → leaves the network

## Project Structure

```
nrf52833-sensor/
├── .cargo/config.toml   # Target, runner (probe-rs), DEFMT_LOG level
├── Cargo.toml            # Dependencies (embassy-nrf 0.3 nrf52833, zigbee-rs crates)
├── build.rs              # Linker script flags (-Tlink.x -Tdefmt.x)
├── memory.x              # Memory layout: 512 KB Flash, 128 KB RAM
└── src/
    └── main.rs           # Async entry point (#[embassy_executor::main])
```
