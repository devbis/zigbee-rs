//! IAS Zone cluster (0x0500).

use crate::attribute::{AttributeAccess, AttributeDefinition, AttributeStore};
use crate::clusters::{AttributeStoreAccess, AttributeStoreMutAccess, Cluster};
use crate::data_types::{ZclDataType, ZclValue};
use crate::{AttributeId, ClusterId, CommandId, ZclStatus};

// Attribute IDs
pub const ATTR_ZONE_STATE: AttributeId = AttributeId(0x0000);
pub const ATTR_ZONE_TYPE: AttributeId = AttributeId(0x0001);
pub const ATTR_ZONE_STATUS: AttributeId = AttributeId(0x0002);
pub const ATTR_IAS_CIE_ADDRESS: AttributeId = AttributeId(0x0010);
pub const ATTR_ZONE_ID: AttributeId = AttributeId(0x0011);
pub const ATTR_NUM_ZONE_SENSITIVITY_LEVELS: AttributeId = AttributeId(0x0012);
pub const ATTR_CURRENT_ZONE_SENSITIVITY_LEVEL: AttributeId = AttributeId(0x0013);

// Zone state values
pub const ZONE_STATE_NOT_ENROLLED: u8 = 0x00;
pub const ZONE_STATE_ENROLLED: u8 = 0x01;

// Zone type values
pub const ZONE_TYPE_STANDARD_CIE: u16 = 0x0000;
pub const ZONE_TYPE_MOTION_SENSOR: u16 = 0x000D;
pub const ZONE_TYPE_CONTACT_SWITCH: u16 = 0x0015;
pub const ZONE_TYPE_FIRE_SENSOR: u16 = 0x0028;
pub const ZONE_TYPE_WATER_SENSOR: u16 = 0x002A;
pub const ZONE_TYPE_CO_SENSOR: u16 = 0x002B;
pub const ZONE_TYPE_PERSONAL_EMERGENCY: u16 = 0x002D;
pub const ZONE_TYPE_REMOTE_CONTROL: u16 = 0x010F;
pub const ZONE_TYPE_KEY_FOB: u16 = 0x0115;
pub const ZONE_TYPE_KEYPAD: u16 = 0x021D;
pub const ZONE_TYPE_STANDARD_WARNING: u16 = 0x0225;

// Command IDs (client to server)
pub const CMD_ZONE_ENROLL_RESPONSE: CommandId = CommandId(0x00);
pub const CMD_INITIATE_NORMAL_OP_MODE: CommandId = CommandId(0x01);
pub const CMD_INITIATE_TEST_MODE: CommandId = CommandId(0x02);

// Command IDs (server to client)
pub const CMD_ZONE_STATUS_CHANGE_NOTIFICATION: CommandId = CommandId(0x00);
pub const CMD_ZONE_ENROLL_REQUEST: CommandId = CommandId(0x01);

/// IAS Zone cluster implementation.
pub struct IasZoneCluster {
    store: AttributeStore<10>,
}

impl IasZoneCluster {
    pub fn new(zone_type: u16) -> Self {
        let mut store = AttributeStore::new();
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_ZONE_STATE,
                data_type: ZclDataType::Enum8,
                access: AttributeAccess::ReadOnly,
                name: "ZoneState",
            },
            ZclValue::Enum8(ZONE_STATE_NOT_ENROLLED),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_ZONE_TYPE,
                data_type: ZclDataType::Enum16,
                access: AttributeAccess::ReadOnly,
                name: "ZoneType",
            },
            ZclValue::Enum16(zone_type),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_ZONE_STATUS,
                data_type: ZclDataType::Bitmap16,
                access: AttributeAccess::ReadOnly,
                name: "ZoneStatus",
            },
            ZclValue::Bitmap16(0),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_IAS_CIE_ADDRESS,
                data_type: ZclDataType::IeeeAddr,
                access: AttributeAccess::ReadWrite,
                name: "IAS_CIE_Address",
            },
            ZclValue::IeeeAddr(0),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_ZONE_ID,
                data_type: ZclDataType::U8,
                access: AttributeAccess::ReadOnly,
                name: "ZoneID",
            },
            ZclValue::U8(0xFF),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_NUM_ZONE_SENSITIVITY_LEVELS,
                data_type: ZclDataType::U8,
                access: AttributeAccess::ReadOnly,
                name: "NumberOfZoneSensitivityLevelsSupported",
            },
            ZclValue::U8(2),
        );
        let _ = store.register(
            AttributeDefinition {
                id: ATTR_CURRENT_ZONE_SENSITIVITY_LEVEL,
                data_type: ZclDataType::U8,
                access: AttributeAccess::ReadWrite,
                name: "CurrentZoneSensitivityLevel",
            },
            ZclValue::U8(0),
        );
        Self { store }
    }

    /// Update zone status bits.
    pub fn set_zone_status(&mut self, status: u16) {
        let _ = self
            .store
            .set_raw(ATTR_ZONE_STATUS, ZclValue::Bitmap16(status));
    }

    /// Read the current zone status from the attribute store.
    pub fn get_zone_status(&self) -> u16 {
        match self.store.get(ATTR_ZONE_STATUS) {
            Some(ZclValue::Bitmap16(v)) => *v,
            _ => 0,
        }
    }

    /// Read the current zone ID from the attribute store.
    pub fn get_zone_id(&self) -> u8 {
        match self.store.get(ATTR_ZONE_ID) {
            Some(ZclValue::U8(v)) => *v,
            _ => 0xFF,
        }
    }

    /// Build a Zone Status Change Notification (command ID 0x00, server → client).
    /// Returns the payload bytes: zone_status(2) + extended_status(1) + zone_id(1) + delay(2)
    pub fn build_zone_status_change_notification(&self) -> heapless::Vec<u8, 6> {
        let mut buf = heapless::Vec::new();
        let status = self.get_zone_status();
        let _ = buf.extend_from_slice(&status.to_le_bytes());
        let _ = buf.push(0x00); // extended_status
        let zone_id = self.get_zone_id();
        let _ = buf.push(zone_id);
        let _ = buf.extend_from_slice(&0u16.to_le_bytes()); // delay
        buf
    }

    /// Build a Zone Enroll Request (command ID 0x01, server → client).
    /// Returns the payload bytes: zone_type(2) + manufacturer_code(2)
    /// The device sends this to CIE to request enrollment.
    pub fn build_zone_enroll_request(&self, manufacturer_code: u16) -> heapless::Vec<u8, 4> {
        let mut buf = heapless::Vec::new();
        let zone_type = match self.store.get(ATTR_ZONE_TYPE) {
            Some(ZclValue::Enum16(v)) => *v,
            _ => 0,
        };
        let _ = buf.extend_from_slice(&zone_type.to_le_bytes());
        let _ = buf.extend_from_slice(&manufacturer_code.to_le_bytes());
        buf
    }

    /// Check if the zone is enrolled.
    pub fn is_enrolled(&self) -> bool {
        matches!(
            self.store.get(ATTR_ZONE_STATE),
            Some(ZclValue::Enum8(ZONE_STATE_ENROLLED))
        )
    }

    /// Set the CIE IEEE address (written by the CIE during enrollment setup).
    pub fn set_cie_address(&mut self, ieee: u64) {
        let _ = self
            .store
            .set_raw(ATTR_IAS_CIE_ADDRESS, ZclValue::IeeeAddr(ieee));
    }

    /// Whether `src_ieee` is the enrolled CIE (`IAS_CIE_Address`).
    ///
    /// Only the CIE may enroll the zone: the dispatcher SHOULD call
    /// [`Self::handle_enroll_response_from`] (or check this first) instead of
    /// passing a Zone Enroll Response from any source to `handle_command`.
    pub fn is_from_cie(&self, src_ieee: u64) -> bool {
        let cie = self.get_cie_address();
        cie != 0 && cie != u64::MAX && cie == src_ieee
    }

    /// Process a Zone Enroll Response, accepting it only from the CIE whose
    /// address is in `IAS_CIE_Address`. Returns `Err(Failure)` and leaves the
    /// enrollment state untouched otherwise.
    pub fn handle_enroll_response_from(
        &mut self,
        src_ieee: u64,
        payload: &[u8],
    ) -> Result<(), ZclStatus> {
        if !self.is_from_cie(src_ieee) {
            return Err(ZclStatus::Failure);
        }
        self.handle_command(CMD_ZONE_ENROLL_RESPONSE, payload)
            .map(|_| ())
    }

    fn sensitivity_supported(&self, level: u8) -> bool {
        match self.store.get(ATTR_NUM_ZONE_SENSITIVITY_LEVELS) {
            Some(ZclValue::U8(n)) => level < *n,
            _ => level == 0,
        }
    }

    /// Get the CIE IEEE address.
    pub fn get_cie_address(&self) -> u64 {
        match self.store.get(ATTR_IAS_CIE_ADDRESS) {
            Some(ZclValue::IeeeAddr(v)) => *v,
            _ => 0,
        }
    }
}

impl AttributeStoreMutAccess for IasZoneCluster {
    fn set(&mut self, id: AttributeId, value: ZclValue) -> Result<(), ZclStatus> {
        AttributeStoreMutAccess::validate_set(self, id, &value)?;
        self.store.set(id, value)
    }
    fn set_raw(&mut self, id: AttributeId, value: ZclValue) -> Result<(), ZclStatus> {
        self.store.set_raw(id, value)
    }
    fn find(&self, id: AttributeId) -> Option<&AttributeDefinition> {
        self.store.find(id)
    }
    fn validate_set(&self, id: AttributeId, value: &ZclValue) -> Result<(), ZclStatus> {
        self.store.validate_set(id, value)?;
        match (id, value) {
            (ATTR_CURRENT_ZONE_SENSITIVITY_LEVEL, ZclValue::U8(v))
                if !self.sensitivity_supported(*v) =>
            {
                Err(ZclStatus::InvalidValue)
            }
            _ => Ok(()),
        }
    }
}

impl Cluster for IasZoneCluster {
    fn cluster_id(&self) -> ClusterId {
        ClusterId::IAS_ZONE
    }

    fn handle_command(
        &mut self,
        cmd_id: CommandId,
        payload: &[u8],
    ) -> Result<heapless::Vec<u8, 64>, ZclStatus> {
        match cmd_id {
            CMD_ZONE_ENROLL_RESPONSE => {
                if payload.len() < 2 {
                    return Err(ZclStatus::MalformedCommand);
                }
                // Success (0x00) enrolls with the given ZoneID; any other
                // code (not supported / no enroll permit / too many zones)
                // leaves the zone un-enrolled (ZCL r8 §8.2.2.3.1).
                let (state, zone_id) = if payload[0] == 0x00 {
                    (ZONE_STATE_ENROLLED, payload[1])
                } else {
                    (ZONE_STATE_NOT_ENROLLED, 0xFF)
                };
                let _ = self.store.set_raw(ATTR_ZONE_STATE, ZclValue::Enum8(state));
                let _ = self.store.set_raw(ATTR_ZONE_ID, ZclValue::U8(zone_id));
                Ok(heapless::Vec::new())
            }
            CMD_INITIATE_NORMAL_OP_MODE => Ok(heapless::Vec::new()),
            CMD_INITIATE_TEST_MODE => {
                // Payload: test mode duration (u8) + current zone sensitivity level (u8)
                if payload.len() < 2 {
                    return Err(ZclStatus::MalformedCommand);
                }
                let _test_mode_duration = payload[0];
                // Only supported sensitivity levels are applied; an
                // unsupported level keeps the current one.
                if self.sensitivity_supported(payload[1]) {
                    let _ = self.store.set_raw(
                        ATTR_CURRENT_ZONE_SENSITIVITY_LEVEL,
                        ZclValue::U8(payload[1]),
                    );
                }
                Ok(heapless::Vec::new())
            }
            _ => Err(ZclStatus::UnsupClusterCommand),
        }
    }

    fn received_commands(&self) -> heapless::Vec<u8, 32> {
        let mut v = heapless::Vec::new();
        let _ = v.push(CMD_ZONE_ENROLL_RESPONSE.0);
        let _ = v.push(CMD_INITIATE_NORMAL_OP_MODE.0);
        let _ = v.push(CMD_INITIATE_TEST_MODE.0);
        v
    }

    fn generated_commands(&self) -> heapless::Vec<u8, 32> {
        let mut v = heapless::Vec::new();
        let _ = v.push(CMD_ZONE_STATUS_CHANGE_NOTIFICATION.0);
        let _ = v.push(CMD_ZONE_ENROLL_REQUEST.0);
        v
    }

    fn attributes(&self) -> &dyn AttributeStoreAccess {
        &self.store
    }
    fn attributes_mut(&mut self) -> &mut dyn AttributeStoreMutAccess {
        self
    }

    /// `ZoneState`/`ZoneID`/`IAS_CIE_Address` describe the CIE enrollment
    /// relationship — analogous to a binding — and MUST NOT be cleared by a
    /// Basic cluster reset; only a full BDB factory reset (which also
    /// leaves the network) may un-enroll the zone. `ZoneStatus` is a live
    /// alarm-condition reading fed by the driver via `set_zone_status`, and
    /// `ZoneType` is a physical-capability constructor parameter.
    /// `CurrentZoneSensitivityLevel` is the only attribute reset here.
    fn reset_to_factory_defaults(&mut self) {
        let _ = self
            .store
            .set_raw(ATTR_CURRENT_ZONE_SENSITIVITY_LEVEL, ZclValue::U8(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CIE: u64 = 0x00AA_BBCC_DDEE_FF01;

    fn state(z: &IasZoneCluster) -> (Option<&ZclValue>, Option<&ZclValue>) {
        (
            z.attributes().get(ATTR_ZONE_STATE),
            z.attributes().get(ATTR_ZONE_ID),
        )
    }

    #[test]
    fn enroll_response_is_accepted_only_from_the_cie() {
        let mut z = IasZoneCluster::new(ZONE_TYPE_CONTACT_SWITCH);
        // No CIE address configured yet.
        assert_eq!(
            z.handle_enroll_response_from(CIE, &[0, 7]),
            Err(ZclStatus::Failure)
        );
        z.set_cie_address(CIE);
        assert_eq!(
            z.handle_enroll_response_from(CIE + 1, &[0, 7]),
            Err(ZclStatus::Failure)
        );
        assert!(!z.is_enrolled());
        z.handle_enroll_response_from(CIE, &[0, 7]).unwrap();
        assert!(z.is_enrolled());
        assert_eq!(z.get_zone_id(), 7);
    }

    #[test]
    fn failed_enroll_response_unenrolls() {
        let mut z = IasZoneCluster::new(ZONE_TYPE_CONTACT_SWITCH);
        z.handle_command(CMD_ZONE_ENROLL_RESPONSE, &[0, 7]).unwrap();
        assert!(z.is_enrolled());
        z.handle_command(CMD_ZONE_ENROLL_RESPONSE, &[0x02, 9])
            .unwrap();
        assert_eq!(
            state(&z),
            (
                Some(&ZclValue::Enum8(ZONE_STATE_NOT_ENROLLED)),
                Some(&ZclValue::U8(0xFF))
            )
        );
    }

    #[test]
    fn sensitivity_level_is_limited_to_supported_levels() {
        let mut z = IasZoneCluster::new(ZONE_TYPE_MOTION_SENSOR);
        // Default: 2 levels supported (0, 1).
        z.handle_command(CMD_INITIATE_TEST_MODE, &[10, 5]).unwrap();
        assert_eq!(
            z.attributes().get(ATTR_CURRENT_ZONE_SENSITIVITY_LEVEL),
            Some(&ZclValue::U8(0))
        );
        z.handle_command(CMD_INITIATE_TEST_MODE, &[10, 1]).unwrap();
        assert_eq!(
            z.attributes().get(ATTR_CURRENT_ZONE_SENSITIVITY_LEVEL),
            Some(&ZclValue::U8(1))
        );
        assert_eq!(
            z.attributes_mut()
                .set(ATTR_CURRENT_ZONE_SENSITIVITY_LEVEL, ZclValue::U8(2)),
            Err(ZclStatus::InvalidValue)
        );
        z.attributes_mut()
            .set(ATTR_CURRENT_ZONE_SENSITIVITY_LEVEL, ZclValue::U8(0))
            .unwrap();
    }
}
