# Telink B91 Zigbee Temperature Sensor

A `no_std` Zigbee 3.0 end device firmware for the **Telink B91** (RISC-V),
reporting temperature and humidity via ZCL clusters 0x0402 and 0x0405.

## Hardware

- **MCU:** Telink B91 — RISC-V 32-bit, 512KB Flash, 256KB SRAM
- **Radio:** Built-in IEEE 802.15.4 + BLE 5.0
- **Board:** Telink B91 devboard (TLSR9518)
- **Button:** GPIO2 — join/leave network
- **LEDs:** GPIO3 (green), GPIO4 (blue)

## Prerequisites

- Rust nightly with `riscv32imc-unknown-none-elf` target

```bash
rustup target add riscv32imc-unknown-none-elf
```

## Vendor Library Setup

The B91 radio driver uses FFI bindings to Telink's precompiled driver library
from the **Telink Zigbee SDK** (`tl_zigbee_sdk`).

### Download the SDK

```bash
git clone https://github.com/telink-semi/tl_zigbee_sdk.git
```

### Set the environment variable

```bash
export TELINK_SDK_DIR=/path/to/tl_zigbee_sdk
```

### Libraries linked by `build.rs`

| Library              | SDK Path              | Purpose                     |
|----------------------|-----------------------|-----------------------------|
| `libdrivers_b91.a`  | `platform/lib/`       | B91 hardware drivers (RF, GPIO, timer, etc.) |

The build script links from `$TELINK_SDK_DIR/platform/lib/`.

## Building

### Stubs build (CI — no Telink SDK required)

```bash
cd examples/telink-b91-sensor
cargo build --release --features stubs
```

The `stubs` feature provides no-op implementations of all FFI symbols.

### Real build (with Telink SDK)

```bash
cd examples/telink-b91-sensor
TELINK_SDK_DIR=/path/to/tl_zigbee_sdk cargo build --release
```

## Flashing

Use the **Telink Burning & Debug Tool (BDT)** or a compatible SWD debugger:

```bash
# With Telink BDT (Windows/Linux)
TelinkBDT --chip b91 --firmware target/riscv32imc-unknown-none-elf/release/telink-b91-sensor
```

## What It Demonstrates

- Zigbee 3.0 end device on Telink B91 with Embassy async runtime
- IEEE 802.15.4 radio via Telink driver library FFI
- Button-driven network join/leave with edge detection
- LED status indication
- ZCL Temperature Measurement + Relative Humidity clusters

## Project Structure

```
telink-b91-sensor/
├── .cargo/config.toml   # Target: riscv32imc-unknown-none-elf, build-std
├── Cargo.toml            # Dependencies, stubs feature flag
├── build.rs              # Telink SDK library linking via TELINK_SDK_DIR
├── memory.x              # Flash @ 0x20000000, RAM @ 0x00000000
└── src/
    ├── main.rs           # Entry point, device setup, sensor loop
    └── stubs.rs          # No-op FFI stubs for CI builds
```
