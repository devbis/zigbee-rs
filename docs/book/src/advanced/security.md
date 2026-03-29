# Security

Zigbee uses a layered security model to protect data in transit. zigbee-rs
implements two of the three layers — NWK-level encryption (shared network key)
and APS-level encryption (per-device link keys). MAC-level security is **not**
used for normal Zigbee 3.0 data frames.

---

## Security Model Overview

```text
┌────────────────────────────────────────────┐
│                APS Layer                    │  Optional end-to-end encryption
│  APS link keys (per device pair)            │  between two specific devices.
├────────────────────────────────────────────┤
│                NWK Layer                    │  Mandatory hop-by-hop encryption
│  Network key (shared by all devices)        │  for ALL routed frames.
├────────────────────────────────────────────┤
│                MAC Layer                    │  NOT used in Zigbee 3.0 for
│  (unused for normal Zigbee data frames)     │  data frames — only for beacons.
└────────────────────────────────────────────┘
```

**NWK security** is always on. Every frame routed through the mesh is encrypted
with the shared network key and authenticated with a 4-byte MIC (Message
Integrity Code).

**APS security** is optional and provides end-to-end confidentiality between
two specific devices. It's used for sensitive operations like network key
transport and can also be used for application-level data.

---

## NWK Security

The NWK security implementation lives in `zigbee_nwk::security`.

### Network Key

All devices on a Zigbee network share the same 128-bit AES network key. The
coordinator generates it during network formation; joining devices receive it
(encrypted) from the Trust Center.

```rust
pub type AesKey = [u8; 16];

pub struct NetworkKeyEntry {
    pub key: AesKey,
    pub seq_number: u8,   // 0–255, rotated on key update
    pub active: bool,
}
```

The stack stores up to `MAX_NETWORK_KEYS` (2) entries — the current active key
and the previous key (kept temporarily during key rotation so in-flight frames
encrypted with the old key can still be decrypted).

```rust
// Set a new network key (moves current key to "previous" slot)
nwk_security.set_network_key(new_key, seq_number);

// Retrieve the active key
let key = nwk_security.active_key().unwrap();

// Look up a key by its sequence number (for decrypting incoming frames)
let key = nwk_security.key_by_seq(frame_key_seq);
```

### AES-128-CCM\* Encryption

Zigbee uses Security Level 5: **ENC-MIC-32** — the payload is encrypted *and*
authenticated with a 4-byte MIC. The implementation uses the RustCrypto `aes`
and `ccm` crates (pure Rust, `#![no_std]`, no allocator):

```rust
type ZigbeeCcm = Ccm<Aes128, U4, U13>;  // M=4 byte MIC, L=2, nonce=13
```

The CCM\* nonce is built from the security auxiliary header:

```text
Nonce (13 bytes) = source_address (8) || frame_counter (4) || security_control (1)
```

> **Spec quirk:** The security level in the over-the-air security control byte
> is always `0` (per Zigbee spec §4.3.1.2). The actual level (`5` = ENC-MIC-32)
> is substituted when building the nonce for encryption/decryption.

### NWK Security Header

Every secured NWK frame carries an auxiliary security header:

```rust
pub struct NwkSecurityHeader {
    pub security_control: u8,       // always 0x2D for Zigbee PRO
    pub frame_counter: u32,         // replay protection
    pub source_address: IeeeAddress, // 64-bit IEEE address of sender
    pub key_seq_number: u8,         // which network key was used
}
```

The constant `NwkSecurityHeader::ZIGBEE_DEFAULT` (`0x2D`) encodes:
- Security Level = 5 (ENC-MIC-32)
- Key Identifier = 1 (Network Key)
- Extended Nonce = 1 (source address present)

### Replay Protection

Each device maintains a per-source frame counter table. Incoming frames are
accepted only if their counter is *strictly greater* than the last seen value
for that source:

```rust
// Step 1: check (before decryption, so we don't waste CPU)
if !nwk_security.check_frame_counter(&source_ieee, frame_counter) {
    // Replay attack — drop the frame
    return;
}

// Step 2: decrypt and verify MIC
let plaintext = nwk_security.decrypt(nwk_hdr, ciphertext, key, &sec_hdr)?;

// Step 3: commit the counter ONLY after successful verification
nwk_security.commit_frame_counter(&source_ieee, frame_counter);
```

The two-phase check-then-commit pattern prevents an attacker from advancing the
counter table with forged frames that fail MIC verification.

---

## APS Security

The APS security implementation lives in `zigbee_aps::security`.

### Key Types

```rust
pub enum ApsKeyType {
    TrustCenterMasterKey    = 0x00,  // pre-installed master key
    TrustCenterLinkKey      = 0x01,  // TC ↔ device link key
    NetworkKey              = 0x02,  // the shared network key
    ApplicationLinkKey      = 0x03,  // app-level key between two devices
    DistributedGlobalLinkKey = 0x04, // for distributed TC networks
}
```

### The Default Trust Center Link Key

Every Zigbee 3.0 device ships with a well-known Trust Center link key
pre-installed:

```rust
/// "ZigBeeAlliance09" in ASCII
pub const DEFAULT_TC_LINK_KEY: [u8; 16] = [
    0x5A, 0x69, 0x67, 0x42, 0x65, 0x65, 0x41, 0x6C,  // ZigBeeAl
    0x6C, 0x69, 0x61, 0x6E, 0x63, 0x65, 0x30, 0x39,  // liance09
];
```

During joining, the Trust Center encrypts the network key with this link key
before sending it to the new device. Because the key is well-known, anyone
within radio range can capture the network key during the join window. For
production deployments, **install codes** provide per-device unique keys.

### APS Security Header

```rust
pub struct ApsSecurityHeader {
    pub security_control: u8,
    pub frame_counter: u32,
    pub source_address: Option<IeeeAddress>,  // if extended nonce bit set
    pub key_seq_number: Option<u8>,           // if Key ID = Network Key
}
```

Security level constants:

| Constant | Value | Meaning |
|----------|-------|---------|
| `SEC_LEVEL_NONE` | 0x00 | No security |
| `SEC_LEVEL_MIC_32` | 0x01 | Auth only, 4-byte MIC |
| `SEC_LEVEL_ENC_MIC_32` | 0x05 | Encrypt + 4-byte MIC (default) |
| `SEC_LEVEL_ENC_MIC_64` | 0x06 | Encrypt + 8-byte MIC |
| `SEC_LEVEL_ENC_MIC_128` | 0x07 | Encrypt + 16-byte MIC |

Key identifier constants:

| Constant | Value | When Used |
|----------|-------|-----------|
| `KEY_ID_DATA_KEY` | 0x00 | Link key (TC or application) |
| `KEY_ID_NETWORK_KEY` | 0x01 | Network key |
| `KEY_ID_KEY_TRANSPORT` | 0x02 | Key-transport key |
| `KEY_ID_KEY_LOAD` | 0x03 | Key-load key |

### Link Key Table

The `ApsSecurity` context manages a table of per-device link keys:

```rust
pub struct ApsSecurity {
    key_table: heapless::Vec<ApsLinkKeyEntry, 16>,  // MAX_KEY_TABLE_ENTRIES = 16
    default_tc_link_key: AesKey,
}

pub struct ApsLinkKeyEntry {
    pub partner_address: IeeeAddress,
    pub key: AesKey,
    pub key_type: ApsKeyType,
    pub outgoing_frame_counter: u32,
    pub incoming_frame_counter: u32,
}
```

Key management methods:

```rust
let mut aps_sec = ApsSecurity::new();

// The default TC link key is pre-loaded
assert_eq!(aps_sec.default_tc_link_key(), &DEFAULT_TC_LINK_KEY);

// Add an application link key for a specific partner
aps_sec.add_key(ApsLinkKeyEntry {
    partner_address: partner_ieee,
    key: my_app_key,
    key_type: ApsKeyType::ApplicationLinkKey,
    outgoing_frame_counter: 0,
    incoming_frame_counter: 0,
})?;

// Look up a key
let entry = aps_sec.find_key(&partner_ieee, ApsKeyType::ApplicationLinkKey);

// Remove a key
aps_sec.remove_key(&partner_ieee, ApsKeyType::ApplicationLinkKey);
```

---

## Network Key Distribution

When a new device joins the network, the Trust Center distributes the network
key through this sequence:

1. **Device sends Association Request** (MAC layer, unencrypted).
2. **Parent router forwards the request** to the Trust Center.
3. **Trust Center encrypts the network key** with the joining device's TC link
   key (either the well-known default or an install-code-derived key).
4. **APS Transport-Key command** carries the encrypted network key to the
   device via its parent router.
5. **Device decrypts the network key** and stores it in NV.
6. **Device sends APS Update-Device** to confirm it's now secured.

After this exchange, the device can encrypt and decrypt NWK frames like all
other nodes on the network.

---

## Install Codes

Install codes provide a per-device unique link key, eliminating the security
weakness of the well-known default key. An install code is:

- A 6, 8, 12, or 16-byte random value printed on the device label
- Combined with a 2-byte CRC-16
- Hashed using Matyas–Meyer–Oseas (MMO) to derive a unique 128-bit link key
- Pre-provisioned on the Trust Center *before* the device joins

In zigbee-rs, install code support is declared in the `TrustCenter` struct:

```rust
pub struct CoordinatorConfig {
    // ...
    pub require_install_codes: bool,
    // ...
}
```

When `require_install_codes` is `true`, the Trust Center only accepts joins
from devices whose IEEE address has a pre-provisioned install-code-derived
key in the link key table.

> **Note:** The current implementation includes a structural placeholder for
> install code derivation. The actual MMO hash computation is not yet
> implemented — only pre-provisioned keys are supported.

---

## Key Rotation

The Trust Center can rotate the network key to limit the exposure window if a
key is compromised:

```rust
// On the Trust Center
trust_center.set_network_key(new_key);  // increments key_seq_number

// The NWK security context keeps both keys during transition
nwk_security.set_network_key(new_key, new_seq);
// keys[0] = new key (active), keys[1] = old key (for in-flight frames)
```

During rotation, the TC broadcasts a **NWK Key-Switch** command. Until all
devices have switched, the network accepts frames encrypted with either the
old or new key (matched by `key_seq_number`).

---

## Summary

| Layer | Key | Scope | MIC Size | Required? |
|-------|-----|-------|----------|-----------|
| NWK | Network key | All devices | 4 bytes | **Yes** (always on) |
| APS | Link key (TC or app) | Two specific devices | 4 bytes | Optional |
| MAC | — | — | — | Not used in Zigbee 3.0 |
