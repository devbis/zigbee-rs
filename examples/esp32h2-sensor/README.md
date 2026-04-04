# ESP32-H2 Zigbee Temperature & Humidity Sensor

A `no_std` Zigbee 3.0 end device for the **ESP32-H2** that reports simulated
temperature and humidity readings. Uses the **esp-hal 1.0** `#[esp_hal::main]`
entry point with `block_on()` for the async runtime.

## Hardware Requirements

- ESP32-H2 development board (built-in IEEE 802.15.4 + BLE 5 radio)
- USB cable for flashing and serial monitor
- BOOT button (GPIO9) used for join/leave control

## Prerequisites

- Rust toolchain with the `esp` channel: `rustup toolchain install esp`
- `espflash` for flashing: `cargo install espflash`
- Target: `riscv32imac-unknown-none-elf` (added automatically via `.cargo/config.toml`)

No vendor libraries or binary blobs are needed — the project uses the
`esp-ieee802154` crate for native 802.15.4 radio access.

## Build

```sh
cargo +esp build -Z build-std=core,alloc --target riscv32imac-unknown-none-elf --release
```

## Flash & Monitor

```sh
espflash flash --monitor target/riscv32imac-unknown-none-elf/release/esp32h2-sensor
```

Or use the configured runner:

```sh
cargo +esp run -Z build-std=core,alloc --target riscv32imac-unknown-none-elf --release
```

## What It Demonstrates

- Initialising the ESP32-H2 IEEE 802.15.4 radio with `esp-radio`
- Building a Zigbee device with the `ZigbeeDevice` builder API
- **esp-hal 1.0 pattern** — `#[esp_hal::main]` entry point with `block_on()` async runtime
  (replaces the removed `embassy_executor` / `riscv_rt` approach)
- Registering ZCL endpoint 1 (Home Automation profile, device type 0x0302)
  with **Basic**, **Power Configuration**, **Identify**, **Temperature Measurement**,
  and **Relative Humidity** clusters
- **NWK Leave handler** — auto-rejoins when coordinator sends Leave
- **Default reporting configuration** — temp/humidity: 60–300 s, battery: 300–3600 s
- **Identify cluster** (0x0003) — LED blinks during Identify
- Button-driven network join/leave via `UserAction::Toggle`
- Periodic simulated sensor updates

> **Note:** NV flash storage is not yet implemented for ESP32-H2. Network state
> is lost on reboot and the device must re-pair.

## Operation

1. Power on → device starts idle
2. Press BOOT → joins the nearest open Zigbee network
3. Once joined → reports simulated sensor values periodically
4. Press BOOT again → leaves the network

## Project Structure

```
esp32h2-sensor/
├── .cargo/config.toml   # Target, runner, rustflags, build-std
├── Cargo.toml            # Dependencies (esp-hal 1.0, esp-radio 0.17, zigbee-rs crates)
└── src/
    └── main.rs           # Application entry point (#[esp_hal::main])
```
