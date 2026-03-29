# NV Storage

Zigbee devices must survive power cycles and reboots without losing their
network membership, keys, or application state. The `zigbee-runtime` crate
defines a `NvStorage` trait that platform backends implement using their
specific flash, EEPROM, or NVS hardware.

---

## The NvStorage Trait

The `NvStorage` trait lives in `zigbee_runtime::nv_storage` and provides six
methods:

```rust
pub trait NvStorage {
    /// Read an item from NV storage.
    /// Returns the number of bytes read into `buf`.
    fn read(&self, id: NvItemId, buf: &mut [u8]) -> Result<usize, NvError>;

    /// Write an item to NV storage.
    fn write(&mut self, id: NvItemId, data: &[u8]) -> Result<(), NvError>;

    /// Delete an item from NV storage.
    fn delete(&mut self, id: NvItemId) -> Result<(), NvError>;

    /// Check if an item exists.
    fn exists(&self, id: NvItemId) -> bool;

    /// Get the length of a stored item.
    fn item_length(&self, id: NvItemId) -> Result<usize, NvError>;

    /// Compact/defragment the storage (if applicable).
    fn compact(&mut self) -> Result<(), NvError>;
}
```

All methods are synchronous — flash writes on embedded targets are typically
blocking and complete in microseconds to low milliseconds. The trait does not
require `alloc`; buffers are caller-provided.

### NvError

```rust
pub enum NvError {
    NotFound,        // Item does not exist
    Full,            // Storage is full — call compact() or free items
    BufferTooSmall,  // Caller buffer too small for the stored item
    HardwareError,   // Flash/EEPROM write or erase failed
    Corrupt,         // CRC or consistency check failed
}
```

---

## NvItemId — What Gets Persisted

Every stored item is identified by an `NvItemId`, a `#[repr(u16)]` enum.
Items are organized into logical groups:

```rust
#[repr(u16)]
pub enum NvItemId {
    // ── Network parameters (0x0001 – 0x000B) ──
    NwkPanId            = 0x0001,
    NwkChannel          = 0x0002,
    NwkShortAddress     = 0x0003,
    NwkExtendedPanId    = 0x0004,
    NwkIeeeAddress      = 0x0005,
    NwkKey              = 0x0006,
    NwkKeySeqNum        = 0x0007,
    NwkFrameCounter     = 0x0008,
    NwkDepth            = 0x0009,
    NwkParentAddress    = 0x000A,
    NwkUpdateId         = 0x000B,

    // ── APS parameters (0x0020 – 0x0023) ──
    ApsTrustCenterAddress = 0x0020,
    ApsLinkKey            = 0x0021,
    ApsBindingTable       = 0x0022,
    ApsGroupTable         = 0x0023,

    // ── BDB commissioning (0x0040 – 0x0044) ──
    BdbNodeIsOnNetwork       = 0x0040,
    BdbCommissioningMode     = 0x0041,
    BdbPrimaryChannelSet     = 0x0042,
    BdbSecondaryChannelSet   = 0x0043,
    BdbCommissioningGroupId  = 0x0044,

    // ── Application data (0x0100+) ──
    AppEndpoint1    = 0x0100,
    AppEndpoint2    = 0x0101,
    AppEndpoint3    = 0x0102,
    AppCustomBase   = 0x0200,   // user-defined items start here
}
```

### What Each Group Contains

| Group | Items | Why It Matters |
|-------|-------|---------------|
| **Network** | PAN ID, channel, addresses, network key, frame counter | Without these the device would have to rejoin the network from scratch. |
| **APS** | TC address, link keys, binding table, group table | Link keys enable encrypted communication; bindings control where reports go. |
| **BDB** | On-network flag, channel sets, commissioning state | Lets the stack know whether to commission or resume on next boot. |
| **Application** | Endpoint-specific attribute data | Preserves user-visible state (e.g., thermostat setpoint, light on/off). |

> **Frame counter persistence is critical.** If `NwkFrameCounter` is lost on
> reboot, the device will transmit frames with a counter of zero. Other devices
> will treat these as replay attacks and silently drop them.

---

## RamNvStorage — In-Memory Storage for Testing

For host-based tests and simulations, `RamNvStorage` implements `NvStorage`
using `heapless` collections — no flash hardware needed:

```rust
pub struct RamNvStorage {
    items: heapless::Vec<NvItem, 64>,  // up to 64 items
}

struct NvItem {
    id: NvItemId,
    data: heapless::Vec<u8, 128>,      // up to 128 bytes per item
}
```

Usage:

```rust
use zigbee_runtime::nv_storage::{RamNvStorage, NvStorage, NvItemId};

let mut nv = RamNvStorage::new();

// Write the network channel
nv.write(NvItemId::NwkChannel, &[15]).unwrap();

// Read it back
let mut buf = [0u8; 4];
let len = nv.read(NvItemId::NwkChannel, &mut buf).unwrap();
assert_eq!(&buf[..len], &[15]);

// Check existence
assert!(nv.exists(NvItemId::NwkChannel));
assert!(!nv.exists(NvItemId::NwkKey));
```

`RamNvStorage` is volatile — all data is lost when the process exits. Its
`compact()` method is a no-op since RAM doesn't suffer from flash wear.

---

## Implementing Flash-Backed NV Storage

To run on real hardware you need a `NvStorage` implementation that writes to
non-volatile memory. Here is the pattern for a typical flash-backed store:

```rust
use zigbee_runtime::nv_storage::{NvStorage, NvItemId, NvError};

pub struct FlashNvStorage {
    // Platform-specific flash handle
    flash: MyFlashDriver,
    // Base address of the NV partition
    base_addr: u32,
    // Simple item index kept in RAM for fast lookup
    index: heapless::Vec<FlashItem, 64>,
}

struct FlashItem {
    id: NvItemId,
    offset: u32,   // byte offset from base_addr
    length: u16,
}

impl NvStorage for FlashNvStorage {
    fn read(&self, id: NvItemId, buf: &mut [u8]) -> Result<usize, NvError> {
        let item = self.index.iter()
            .find(|i| i.id == id)
            .ok_or(NvError::NotFound)?;
        if buf.len() < item.length as usize {
            return Err(NvError::BufferTooSmall);
        }
        self.flash.read(self.base_addr + item.offset, &mut buf[..item.length as usize])
            .map_err(|_| NvError::HardwareError)?;
        Ok(item.length as usize)
    }

    fn write(&mut self, id: NvItemId, data: &[u8]) -> Result<(), NvError> {
        // Append-only: write to next free sector, update index
        // On compact(), defragment and reclaim deleted entries
        todo!("platform-specific flash write")
    }

    fn delete(&mut self, id: NvItemId) -> Result<(), NvError> {
        // Mark the item as deleted in flash; reclaim space on compact()
        todo!("platform-specific flash delete")
    }

    fn exists(&self, id: NvItemId) -> bool {
        self.index.iter().any(|i| i.id == id)
    }

    fn item_length(&self, id: NvItemId) -> Result<usize, NvError> {
        self.index.iter()
            .find(|i| i.id == id)
            .map(|i| i.length as usize)
            .ok_or(NvError::NotFound)
    }

    fn compact(&mut self) -> Result<(), NvError> {
        // Erase + rewrite: copy live items to a scratch sector,
        // erase the primary sector, copy back.
        todo!("wear-leveled compaction")
    }
}
```

### Platform Hints

The source code documents these target backends:

| Platform | Recommended Backend |
|----------|-------------------|
| ESP32 | `nvs_flash` partition (key-value store built into ESP-IDF) |
| nRF52840 | `nrf-softdevice` flash pages or littlefs |
| STM32WB | Internal flash with wear leveling |
| Generic | Bridge via the `embedded-storage` traits |

---

## What State Is Saved and Restored on Reboot

When the stack starts, it checks `BdbNodeIsOnNetwork`. If the flag is set, it
restores the following items from NV instead of starting fresh commissioning:

1. **Network identity** — `NwkPanId`, `NwkChannel`, `NwkShortAddress`,
   `NwkExtendedPanId`
2. **Security material** — `NwkKey`, `NwkKeySeqNum`, `NwkFrameCounter`,
   `ApsLinkKey`, `ApsTrustCenterAddress`
3. **Topology** — `NwkParentAddress`, `NwkDepth`, `NwkUpdateId`
4. **Bindings and groups** — `ApsBindingTable`, `ApsGroupTable`
5. **Application attributes** — `AppEndpoint1`–`AppEndpoint3` and any
   `AppCustomBase + N` items the application registered

If any critical item is missing or corrupt (`NvError::Corrupt`), the stack
falls back to a fresh commissioning cycle — the device will rejoin the network
as if it were new.

### Saving Before Deep Sleep

Before entering deep sleep (which typically resets the CPU), the runtime
persists all dirty state:

```rust
// Pseudocode from the event loop
if let SleepDecision::DeepSleep(_) = decision {
    nv.write(NvItemId::NwkFrameCounter, &fc.to_le_bytes())?;
    nv.write(NvItemId::NwkShortAddress, &addr.0.to_le_bytes())?;
    // ... save any changed application attributes ...
}
```

> **Tip:** Only write items that have actually changed since the last save.
> Flash has limited write endurance (typically 10,000–100,000 cycles per
> sector), so unnecessary writes shorten the device's lifetime.
