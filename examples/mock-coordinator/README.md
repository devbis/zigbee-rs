# Mock Zigbee Coordinator

Simulates a Zigbee coordinator that forms a network, configures the Trust Center,
and accepts joining devices — all on the host machine using `MockMac`.

## What It Demonstrates

- **Energy detection scan** — MLME-SCAN (ED) to find the quietest channel
- **Network formation** — MLME-START as PAN coordinator on the selected channel
- **Trust Center** — `TrustCenter` with default link key (`ZigBeeAlliance09`), network key generation
- **Coordinator logic** — `Coordinator` with address allocation, child capacity, permit joining
- **DeviceBuilder** — coordinator device with Basic + Identify clusters (HA profile)

## Build & Run

```sh
cargo run -p mock-coordinator
```

## Expected Output

1. Initializes MockMac with energy scan results for channels 11, 15, 20, 25
2. Runs energy detection scan — selects channel 15 (lowest energy = 45)
3. Forms network: PAN `0x1A62`, short address `0x0000`, non-beacon mode
4. Configures Trust Center with generated network key and default TC link key
5. Simulates 3 devices joining (Temp/Humidity Sensor, Dimmable Light, Smart Plug)
6. Builds coordinator runtime device and prints network summary

## Project Structure

```
mock-coordinator/
├── Cargo.toml      # Dependencies: zigbee-*, pollster
└── src/
    └── main.rs     # Coordinator simulation (~270 lines)
```

## Dependencies

| Crate | Purpose |
|---|---|
| `zigbee` | `Coordinator`, `TrustCenter`, `CoordinatorConfig` |
| `zigbee-mac` (mock) | MockMac, energy scan, PIB management |
| `zigbee-nwk` | `DeviceType::Coordinator` |
| `zigbee-runtime` | `DeviceBuilder` |
| `zigbee-types` | IeeeAddress, PanId, ChannelMask, ShortAddress |
| `pollster` | Block on async MAC calls |
