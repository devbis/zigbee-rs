# Telink TLSR8258 Zigbee Temperature Sensor

A `no_std` Zigbee 3.0 end device firmware for the **Telink TLSR8258** (tc32 ISA),
reporting temperature and humidity via ZCL clusters 0x0402 and 0x0405.

## Hardware

- **MCU:** Telink TLSR8258 — tc32 core, 512KB Flash, 64KB SRAM
- **Radio:** Built-in IEEE 802.15.4 + BLE
- **Boards:** Sonoff SNZB-02, Tuya Zigbee sensors, IKEA devices, Telink devboard
- **Button:** GPIO2 — join/leave network
- **LED:** GPIO3

## Prerequisites

- Rust nightly with `thumbv6m-none-eabi` target (compilation stand-in)

```bash
rustup target add thumbv6m-none-eabi
```

## ⚠️ Note on tc32 ISA

The TLSR8258 uses Telink's **proprietary tc32 instruction set**. There is no
official Rust target for tc32. For `cargo check` / `cargo build`, we use
`thumbv6m-none-eabi` as a compilation stand-in to verify the Rust code compiles
and the Zigbee stack logic is correct.

Real production builds require the **Telink tc32 GCC toolchain** and would
typically link via a C firmware project that calls into a Rust static library.

## Vendor Library Setup

The TLSR8258 driver uses FFI bindings to Telink's precompiled driver library
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

| Library               | SDK Path              | Purpose                         |
|-----------------------|-----------------------|---------------------------------|
| `libdrivers_8258.a`  | `platform/lib/`       | TLSR8258 hardware drivers (RF, GPIO, timer, etc.) |

The build script links from `$TELINK_SDK_DIR/platform/lib/`.

## Building

### Stubs build (CI — no Telink SDK required)

```bash
cd examples/telink-tlsr8258-sensor
cargo build --release --features stubs
```

The `stubs` feature provides no-op implementations of all FFI symbols.

### Real build (with Telink SDK)

```bash
cd examples/telink-tlsr8258-sensor
TELINK_SDK_DIR=/path/to/tl_zigbee_sdk cargo build --release
```

> **Note:** This produces a `thumbv6m` ELF binary for verification purposes.
> For production tc32 firmware, integrate the Rust code as a static library
> into a Telink tc32 GCC project.

## Flashing

Use the **Telink Burning & Debug Tool (BDT)** with a Telink USB programmer:

```bash
# With Telink BDT
TelinkBDT --chip 8258 --firmware <your_tc32_firmware.bin>
```

For the `thumbv6m` stand-in build, the binary is not directly flashable to
real TLSR8258 hardware.

## What It Demonstrates

- Zigbee 3.0 end device targeting the popular TLSR8258 platform
- Embassy async runtime with tc32-compatible compilation
- IEEE 802.15.4 radio via Telink driver library FFI
- Button-driven network join/leave with edge detection
- ZCL Temperature Measurement + Relative Humidity clusters
- Cross-compilation approach for non-standard ISA targets

## What Works vs. What's Stubbed

### ✅ Implemented
- **Time driver**: Reads the TLSR8258 32-bit system timer at `0x740`, extends
  to 64-bit with wraparound detection, converts to microseconds at 16 MHz
- **GPIO**: Real register-mapped I/O for group A (input, output, pull-up)
- **RF ISR routing**: Dispatch function for RF interrupt → MAC driver callbacks
- **Sleep**: WFI instruction for light sleep; `light_sleep_ms()` placeholder
  for Telink PM driver integration
- **MAC driver**: Full Telink MAC with CSMA-CA, ED scan, indirect TX queue,
  frame-pending bit, poll support

### 🔧 Requires real hardware / SDK for full functionality
- **Sensor data**: Synthetic temperature/humidity (no I²C sensor driver yet)
- **Timer alarm**: `schedule_wake()` not yet wired to hardware compare
  interrupt — Embassy uses polling mode
- **Deep sleep**: `light_sleep_ms()` falls back to WFI; real suspend requires
  Telink PM_LowPwrEnter() integration
- **Build target**: Uses `thumbv6m-none-eabi` as stand-in for tc32

## Project Structure

```
telink-tlsr8258-sensor/
├── .cargo/config.toml   # Target: thumbv6m-none-eabi (tc32 stand-in), build-std
├── Cargo.toml            # Dependencies, stubs feature flag
├── build.rs              # Telink SDK library linking via TELINK_SDK_DIR
├── memory.x              # Flash @ 0x00000000, RAM @ 0x00840000
└── src/
    ├── main.rs           # Entry point, device setup, sensor loop
    └── stubs.rs          # No-op FFI stubs for CI builds
```
