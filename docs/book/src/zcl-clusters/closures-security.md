# Closures & Security Clusters

Clusters for physical access control (door locks, window coverings) and intrusion detection (IAS zones).

---

## Door Lock (0x0101)

Full-featured door lock with PIN code management, auto-relock, and operating modes.

### Attributes

| Attribute | ID | Type | Access | Description |
|-----------|----|------|--------|-------------|
| LockState | `0x0000` | Enum8 | Report | 0=NotFullyLocked, 1=Locked, 2=Unlocked |
| LockType | `0x0001` | Enum8 | Read | DeadBolt(0), Magnetic(1), Other(2), etc. |
| ActuatorEnabled | `0x0002` | Bool | Read | Actuator operational |
| DoorState | `0x0003` | Enum8 | Report | Open(0), Closed(1), Jammed(2), etc. |
| DoorOpenEvents | `0x0004` | U32 | R/W | Door-open counter |
| DoorClosedEvents | `0x0005` | U32 | R/W | Door-close counter |
| OpenPeriod | `0x0006` | U16 | R/W | Auto-close period |
| NumPINUsersSupported | `0x0012` | U16 | Read | Max PIN users |
| MaxPINCodeLength | `0x0017` | U8 | Read | Max PIN length (default 8) |
| MinPINCodeLength | `0x0018` | U8 | Read | Min PIN length (default 4) |
| Language | `0x0021` | String | R/W | Display language |
| AutoRelockTime | `0x0023` | U32 | R/W | Auto-relock delay in seconds |
| OperatingMode | `0x0025` | Enum8 | R/W | Normal(0), Vacation(1), Privacy(2), etc. |

### Commands (Client → Server)

| ID | Command | Description |
|----|---------|-------------|
| `0x00` | LockDoor | Lock the door |
| `0x01` | UnlockDoor | Unlock (starts auto-relock timer) |
| `0x02` | Toggle | Toggle lock/unlock |
| `0x03` | UnlockWithTimeout | Unlock with auto-relock |
| `0x05` | SetPINCode | Set user PIN |
| `0x06` | GetPINCode | Retrieve user PIN |
| `0x07` | ClearPINCode | Delete a user's PIN |
| `0x08` | ClearAllPINCodes | Delete all PINs |
| `0x09` | SetUserStatus | Enable/disable a user |
| `0x0A` | GetUserStatus | Query user status |

### Auto-Relock

When `AutoRelockTime` is non-zero, unlocking the door starts a countdown. The `tick()` method (call every second) decrements the timer and automatically locks when it expires:

```rust
use zigbee_zcl::clusters::door_lock::DoorLockCluster;

let mut lock = DoorLockCluster::new(0x00); // DeadBolt type
lock.set_lock_state(0x01); // Locked

// When UnlockDoor command arrives, the cluster:
// 1. Sets LockState = Unlocked
// 2. Reads AutoRelockTime attribute
// 3. Starts countdown in auto_relock_remaining

// In your 1-second timer:
lock.tick(); // When countdown reaches 0 → auto-locks
```

### PIN Code Management

PINs are stored in a fixed-capacity table (8 entries):

```rust
// SetPINCode payload: user_id(2) + status(1) + type(1) + pin_len(1) + pin[]
// The cluster stores: { status, user_type, pin: Vec<u8, 8> }

// PinEntry fields:
//   status: 0=available, 1=occupied_enabled, 3=occupied_disabled
//   user_type: 0=unrestricted, 1=year_day, 2=week_day, 3=master
```

---

## Window Covering (0x0102)

Controls roller shades, blinds, awnings, and other window treatments.

### Attributes

| Attribute | ID | Type | Access | Description |
|-----------|----|------|--------|-------------|
| WindowCoveringType | `0x0000` | Enum8 | Read | Covering type |
| ConfigStatus | `0x0007` | Bitmap8 | Read | Configuration flags |
| CurrentPositionLiftPercentage | `0x0008` | U8 | Report | Lift position (0–100%) |
| CurrentPositionTiltPercentage | `0x0009` | U8 | Report | Tilt position (0–100%) |
| InstalledOpenLimitLift | `0x0010` | U16 | Read | Open limit |
| InstalledClosedLimitLift | `0x0011` | U16 | Read | Closed limit |
| Mode | `0x0017` | Bitmap8 | R/W | Operating mode flags |

### Covering Types

| Value | Type |
|-------|------|
| `0x00` | Roller Shade |
| `0x04` | Drapery |
| `0x05` | Awning |
| `0x06` | Shutter |
| `0x07` | Tilt Blind (tilt only) |
| `0x08` | Tilt Blind (lift + tilt) |
| `0x09` | Projector Screen |

### Commands

| ID | Command |
|----|---------|
| `0x00` | UpOpen |
| `0x01` | DownClose |
| `0x02` | Stop |
| `0x05` | GoToLiftPercentage |
| `0x08` | GoToTiltPercentage |

```rust
use zigbee_zcl::clusters::window_covering::WindowCoveringCluster;

let covering = WindowCoveringCluster::new(0x00); // Roller shade
```

---

## IAS Zone (0x0500)

The primary security cluster for intrusion/alarm sensors. Implements a state machine for zone enrollment with a CIE (Control and Indicating Equipment).

### Attributes

| Attribute | ID | Type | Access | Description |
|-----------|----|------|--------|-------------|
| ZoneState | `0x0000` | Enum8 | Read | NotEnrolled(0) / Enrolled(1) |
| ZoneType | `0x0001` | Enum16 | Read | Sensor type code |
| ZoneStatus | `0x0002` | Bitmap16 | Read | Alarm and tamper bits |
| IAS_CIE_Address | `0x0010` | IEEE | R/W | CIE's IEEE address |
| ZoneID | `0x0011` | U8 | Read | Assigned zone ID |
| NumZoneSensitivityLevels | `0x0012` | U8 | Read | Supported sensitivity levels |
| CurrentZoneSensitivityLevel | `0x0013` | U8 | R/W | Active sensitivity |

### Zone Types

| Value | Type |
|-------|------|
| `0x0000` | Standard CIE |
| `0x000D` | Motion Sensor |
| `0x0015` | Contact Switch |
| `0x0028` | Fire Sensor |
| `0x002A` | Water Sensor |
| `0x002B` | CO Sensor |
| `0x002D` | Personal Emergency |
| `0x010F` | Remote Control |
| `0x0115` | Key Fob |
| `0x021D` | Keypad |
| `0x0225` | Standard Warning |

### Zone Status Bits

| Bit | Meaning |
|-----|---------|
| 0 | Alarm1 (zone-type specific) |
| 1 | Alarm2 (zone-type specific) |
| 2 | Tamper |
| 3 | Battery low |
| 4 | Supervision reports |
| 5 | Restore reports |
| 6 | Trouble |
| 7 | AC (mains) fault |

### Enrollment Flow

```
Device                           CIE
  │                               │
  │  Write IAS_CIE_Address ◄──────┤
  │                               │
  ├──── Zone Enroll Request ─────►│
  │     (zone_type + mfr_code)    │
  │                               │
  │  Zone Enroll Response ◄───────┤
  │  (status=0x00, zone_id)       │
  │                               │
  │  [ZoneState → Enrolled]       │
  │                               │
  ├── Zone Status Change Notif ──►│
  │   (alarm bits + zone_id)      │
```

### Usage

```rust
use zigbee_zcl::clusters::ias_zone::*;

// Motion sensor
let mut zone = IasZoneCluster::new(ZONE_TYPE_MOTION_SENSOR);

// CIE writes its address during setup:
zone.set_cie_address(0x00124B0012345678);

// Build enrollment request to send to CIE:
let enroll_payload = zone.build_zone_enroll_request(0x1234); // mfr code

// After CIE responds with ZoneEnrollResponse(success, zone_id=5):
// handle_command() sets ZoneState=Enrolled, ZoneID=5

// When motion detected:
zone.set_zone_status(0x0001); // Alarm1
let notif = zone.build_zone_status_change_notification();
// Send as cluster-specific command 0x00 (server→client)

// Check enrollment:
if zone.is_enrolled() {
    // Device is enrolled and can send notifications
}
```

---

## IAS ACE (0x0501)

Ancillary Control Equipment — keypads and panic buttons.

```rust
use zigbee_zcl::clusters::ias_ace; // IasAceCluster
```

---

## IAS WD (0x0502)

Warning Device — sirens and strobes.

```rust
use zigbee_zcl::clusters::ias_wd; // IasWdCluster
```
