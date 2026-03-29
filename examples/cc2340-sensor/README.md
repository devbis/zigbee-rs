# CC2340R5 Zigbee Temperature Sensor

A `no_std` Zigbee 3.0 end device firmware for the **TI CC2340R5** (ARM Cortex-M0+),
reporting temperature and humidity via ZCL clusters 0x0402 and 0x0405.

## Hardware

- **MCU:** CC2340R5 — ARM Cortex-M0+, 512KB Flash, 36KB SRAM
- **Radio:** Built-in IEEE 802.15.4 + BLE 5.0
- **Board:** TI LP-EM-CC2340R5 LaunchPad
- **Buttons:** BTN1 (DIO13) = join/leave, BTN2 (DIO14) = identify
- **LEDs:** LED1 (DIO7) = network status, LED2 (DIO6) = activity

## Prerequisites

- Rust nightly with `thumbv6m-none-eabi` target
- TI UniFlash or `openocd` for flashing

```bash
rustup target add thumbv6m-none-eabi
```

## Vendor Library Setup

The CC2340R5 radio driver requires precompiled libraries from the
**TI SimpleLink Low Power F3 SDK**.

### Download the SDK

Download from [TI's website](https://www.ti.com/tool/SIMPLELINK-LOWPOWER-F3-SDK)
or install via TI's Resource Explorer.

### Set the environment variable

```bash
export CC2340_SDK_DIR=/path/to/simplelink_lowpower_f3_sdk_8_xx_xx_xx
```

### Libraries linked by `build.rs`

| Library                    | SDK Path                                              | Purpose                    |
|----------------------------|-------------------------------------------------------|----------------------------|
| `librcl_cc23x0r5.a`       | `source/ti/drivers/rcl/lib/ticlang/m0p/`              | Radio Control Layer        |
| `libpbe_ieee_cc23x0r5.a`  | `source/ti/devices/cc23x0r5/rf_patches/lib/ticlang/m0p/` | PBE firmware patch     |
| `libmce_ieee_cc23x0r5.a`  | same as above                                         | MCE firmware patch         |
| `librfe_ieee_cc23x0r5.a`  | same as above                                         | RFE firmware patch         |

The build script also checks for optional ZBOSS platform libraries at:
`source/third_party/zigbee/libraries/cc2340r5/ticlang/`

## Building

### Stubs build (CI — no TI SDK required)

```bash
cd examples/cc2340-sensor
cargo build --release --features stubs
```

The `stubs` feature provides no-op implementations of all FFI symbols.

### Real build (with TI SDK)

```bash
cd examples/cc2340-sensor
CC2340_SDK_DIR=/path/to/simplelink_lowpower_f3_sdk cargo build --release
```

## Flashing

Use TI UniFlash, `openocd`, or a J-Link debugger:

```bash
# With openocd
openocd -f board/ti_cc2340r5.cfg -c "program target/thumbv6m-none-eabi/release/cc2340-sensor verify reset exit"

# With probe-rs
probe-rs run --chip CC2340R5 target/thumbv6m-none-eabi/release/cc2340-sensor
```

## What It Demonstrates

- Zigbee 3.0 end device on CC2340R5 with Embassy async runtime
- IEEE 802.15.4 radio via TI Radio Control Layer (RCL)
- Button-driven network join/leave with edge detection
- LED status indication (joined/not joined)
- ZCL Temperature Measurement + Relative Humidity clusters

## Project Structure

```
cc2340-sensor/
├── .cargo/config.toml   # Target: thumbv6m-none-eabi, build-std
├── Cargo.toml            # Dependencies, stubs feature flag
├── build.rs              # TI SDK library linking via CC2340_SDK_DIR
├── memory.x              # Flash @ 0x00000000, RAM @ 0x20000000
└── src/
    ├── main.rs           # Entry point, device setup, sensor loop
    └── stubs.rs          # No-op FFI stubs for CI builds
```
