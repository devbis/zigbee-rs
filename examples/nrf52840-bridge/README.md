# nRF52840 USB Serial Bridge (IEEE 802.15.4)

Thin firmware that exposes the **nRF52840 802.15.4 radio** over a USB CDC ACM
serial port. A host-side Zigbee stack (e.g. `zigbee-mac`'s `serial` backend)
sends commands and receives indications through a framed binary protocol,
turning the nRF52840 into a Zigbee coordinator/sniffer dongle.

## Hardware Requirements

- nRF52840-DK (PCA10056) or nRF52840 Dongle (PCA10059)
- USB cable (the device enumerates as a USB CDC ACM serial port)
- Debug probe for flashing (J-Link on-board for DK, or external)

## Prerequisites

- Rust stable toolchain
- `probe-rs`: `cargo install probe-rs-tools`
- Target: `thumbv7em-none-eabihf` (configured in `.cargo/config.toml`)

No vendor libraries, SoftDevice, or binary blobs are needed.

## Build

```sh
cargo build --release
```

## Flash & Run

```sh
probe-rs run --chip nRF52840_xxAA target/thumbv7em-none-eabihf/release/nrf52840-bridge
```

Or use the configured runner:

```sh
cargo run --release
```

## Serial Protocol

The bridge uses a simple framed protocol over USB serial:

```
START(0xF1) | CMD | SEQ | LEN_LO | LEN_HI | PAYLOAD[0..LEN] | CRC_LO | CRC_HI
```

CRC is CRC16-CCITT over `CMD..PAYLOAD` (excludes START byte and CRC itself).

### Supported Commands

| CMD  | Name          | Direction  | Description                      |
|------|---------------|------------|----------------------------------|
| 0x01 | RESET_REQ     | host → fw  | Reset radio, optionally PIB      |
| 0x02 | SCAN_REQ      | host → fw  | Energy/active/passive scan       |
| 0x03 | ASSOCIATE_REQ | host → fw  | Send association request         |
| 0x04 | DATA_REQ      | host → fw  | Transmit raw 802.15.4 frame      |
| 0x05 | SET_REQ       | host → fw  | Set a PIB attribute              |
| 0x06 | GET_REQ       | host → fw  | Get a PIB attribute              |
| 0x07 | START_REQ     | host → fw  | Start PAN (coordinator mode)     |
| 0xC1 | DATA_IND      | fw → host  | Received 802.15.4 frame          |

Each request has a corresponding `_CNF` (confirm) response (CMD | 0x80).

## What It Demonstrates

- USB CDC ACM device with `embassy-usb` and `embassy-usb-serial`
- Async split architecture: separate radio RX and USB TX/RX tasks
  communicating via `embassy_sync::channel::Channel`
- Full IEEE 802.15.4 MAC primitive pass-through (scan, associate, data, PIB)
- CRC16-CCITT frame validation
- Building a coordinator-capable bridge without any Zigbee stack on the MCU

## Project Structure

```
nrf52840-bridge/
├── .cargo/config.toml   # Target, runner (probe-rs), DEFMT_LOG level
├── Cargo.toml            # Dependencies (embassy-nrf, embassy-usb, heapless)
├── build.rs              # Linker script flags (-Tlink.x -Tdefmt.x)
├── memory.x              # Memory layout: 1 MB Flash, 256 KB RAM
└── src/
    └── main.rs           # Async entry point with USB serial + radio tasks
```
