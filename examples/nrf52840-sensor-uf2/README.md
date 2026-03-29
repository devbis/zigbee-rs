# nRF52840 Zigbee Sensor — UF2 Bootloader

An async Embassy-based Zigbee 3.0 end device for the **nRF52840** designed
for boards with a UF2 bootloader (nice!nano, ProMicro nRF52840, MDK Dongle,
Nordic PCA10059). No debug probe required — flash by drag-and-drop.

## Supported Boards

| Feature            | Board                         | LED        | Flash origin |
|--------------------|-------------------------------|------------|-------------|
| `board-promicro`   | ProMicro nRF52840 / nice!nano | P0.15 HIGH | 0x26000     |
| `board-mdk`        | Makerdiary MDK USB Dongle     | P0.22 LOW  | 0x1000      |
| `board-nrf-dongle` | Nordic PCA10059 Dongle        | P0.06 LOW  | 0x1000      |
| `board-nrf-dk`     | Nordic nRF52840 DK (PCA10056) | P0.13 LOW  | 0x0000      |

Default feature: `board-promicro`.

## Hardware Requirements

- One of the supported boards listed above
- USB cable (the board appears as a USB mass-storage device in bootloader mode)

## Prerequisites

- Rust stable toolchain
- Target: `thumbv7em-none-eabihf` (configured in `.cargo/config.toml`)
- `uf2conv.py` for UF2 conversion: `pip install uf2conv`
- (Alternative) `cargo-binutils`: `cargo install cargo-binutils` + `rustup component add llvm-tools`

No vendor libraries or binary blobs are needed.

## Build

```sh
# ProMicro / nice!nano (default):
cargo build --release

# MDK Dongle:
cargo build --release --no-default-features --features board-mdk

# Nordic PCA10059 Dongle:
cargo build --release --no-default-features --features board-nrf-dongle

# Nordic DK (PCA10056):
cargo build --release --no-default-features --features board-nrf-dk
```

## Flash (UF2 drag-and-drop)

1. Convert ELF to binary:
   ```sh
   cargo objcopy --release -- -O binary firmware.bin
   ```
2. Convert to UF2 (adjust `-b` base address per board):
   ```sh
   # ProMicro / nice!nano:
   uf2conv.py -c -f 0xADA52840 -b 0x26000 firmware.bin -o firmware.uf2

   # MDK / Nordic Dongle:
   uf2conv.py -c -f 0xADA52840 -b 0x1000 firmware.bin -o firmware.uf2
   ```
3. Enter bootloader mode (double-tap reset on nice!nano, or hold reset + plug USB)
4. Copy `firmware.uf2` to the USB drive that appears (e.g. `NICENANO`)

## What It Demonstrates

- Multi-board support via Cargo features with board-specific memory layouts
  generated in `build.rs`
- SoftDevice S140 disable via SVC call (ProMicro / nice!nano)
- Auto-join on boot with automatic rejoin every 15 s if not connected
- Fast-poll mode (250 ms) for 60 s after join to support ZHA/Z2M interview
- Sleepy End Device (SED) polling architecture — `device.poll()` instead of
  `device.receive()`
- On-chip TEMP sensor + simulated humidity (reports every 15 s)
- LED status: solid ON = joined, double-blink = searching, OFF = idle
- Button support on DK: short press = toggle join/leave, long press (3 s) = factory reset
- `log` → `defmt` bridge for stack-internal logging

## Operation

1. Power on → LED solid ON for 3 s (boot signal), then auto-joins
2. LED double-blink while searching for a network
3. LED solid ON once joined; sensor reports every 15 s
4. (DK only) Short press Button 1 → toggle join/leave
5. (DK only) Hold Button 1 for 3 s → factory reset (5× LED flash, reboot)

## Project Structure

```
nrf52840-sensor-uf2/
├── .cargo/config.toml   # Target, DEFMT_LOG level (no runner — UF2 flash)
├── Cargo.toml            # Board features, dependencies (embassy-nrf, zigbee-rs)
├── build.rs              # Generates memory.x per board feature, linker flags
└── src/
    └── main.rs           # Async entry point with SED polling loop
```
