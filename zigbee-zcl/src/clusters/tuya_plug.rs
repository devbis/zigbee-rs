//! Tuya manufacturer-specific cluster (0xFC01).
//!
//! Custom attributes for Tuya smart plug configuration:
//! - 0xF000: key_lock — child lock (Bool)
//! - 0xF001: led_control — LED enable (Bool)
//! - 0xF002: current_max — max current in centi-amps (U16)
//! - 0xF003: power_max — max power in watts (U16)
//! - 0xF004: time_reload — auto-restart delay in seconds (U16)
//! - 0xF005: protect_control — overload protection enable (Bool)
//! - 0xF006: auto_restart — auto-restart after overload (Bool)

use crate::attribute::{AttributeAccess, AttributeDefinition, AttributeStore};
use crate::clusters::{AttributeStoreAccess, AttributeStoreMutAccess, Cluster};
use crate::data_types::{ZclDataType, ZclValue};
use crate::{AttributeId, ClusterId, CommandId, ZclStatus};

/// Tuya manufacturer-specific cluster ID.
pub const TUYA_CLUSTER_ID: ClusterId = ClusterId(0xFC01);

// Attribute IDs (manufacturer-specific range 0xF000-0xFFFF)
pub const ATTR_KEY_LOCK: AttributeId = AttributeId(0xF000);
pub const ATTR_LED_CONTROL: AttributeId = AttributeId(0xF001);
pub const ATTR_CURRENT_MAX: AttributeId = AttributeId(0xF002);
pub const ATTR_POWER_MAX: AttributeId = AttributeId(0xF003);
pub const ATTR_TIME_RELOAD: AttributeId = AttributeId(0xF004);
pub const ATTR_PROTECT_CONTROL: AttributeId = AttributeId(0xF005);
pub const ATTR_AUTO_RESTART: AttributeId = AttributeId(0xF006);

/// Tuya manufacturer-specific cluster for smart plug configuration.
pub struct TuyaPlugCluster {
    store: AttributeStore<8>,
}

impl Default for TuyaPlugCluster {
    fn default() -> Self {
        Self::new()
    }
}

impl TuyaPlugCluster {
    pub fn new() -> Self {
        let mut store = AttributeStore::new();

        let _ = store.register(
            AttributeDefinition {
                id: ATTR_KEY_LOCK,
                data_type: ZclDataType::Bool,
                access: AttributeAccess::ReadWrite,
                name: "KeyLock",
            },
            ZclValue::Bool(false),
        );

        let _ = store.register(
            AttributeDefinition {
                id: ATTR_LED_CONTROL,
                data_type: ZclDataType::Bool,
                access: AttributeAccess::ReadWrite,
                name: "LedControl",
            },
            ZclValue::Bool(true),
        );

        let _ = store.register(
            AttributeDefinition {
                id: ATTR_CURRENT_MAX,
                data_type: ZclDataType::U16,
                access: AttributeAccess::ReadWrite,
                name: "CurrentMax",
            },
            ZclValue::U16(1600), // 16.00A default
        );

        let _ = store.register(
            AttributeDefinition {
                id: ATTR_POWER_MAX,
                data_type: ZclDataType::U16,
                access: AttributeAccess::ReadWrite,
                name: "PowerMax",
            },
            ZclValue::U16(3680), // 3680W default (230V * 16A)
        );

        let _ = store.register(
            AttributeDefinition {
                id: ATTR_TIME_RELOAD,
                data_type: ZclDataType::U16,
                access: AttributeAccess::ReadWrite,
                name: "TimeReload",
            },
            ZclValue::U16(30), // 30 seconds default
        );

        let _ = store.register(
            AttributeDefinition {
                id: ATTR_PROTECT_CONTROL,
                data_type: ZclDataType::Bool,
                access: AttributeAccess::ReadWrite,
                name: "ProtectControl",
            },
            ZclValue::Bool(true),
        );

        let _ = store.register(
            AttributeDefinition {
                id: ATTR_AUTO_RESTART,
                data_type: ZclDataType::Bool,
                access: AttributeAccess::ReadWrite,
                name: "AutoRestart",
            },
            ZclValue::Bool(false),
        );

        Self { store }
    }

    /// Check if child lock is enabled.
    pub fn is_key_locked(&self) -> bool {
        matches!(self.store.get(ATTR_KEY_LOCK), Some(ZclValue::Bool(true)))
    }

    /// Check if LED control is enabled.
    pub fn is_led_enabled(&self) -> bool {
        matches!(
            self.store.get(ATTR_LED_CONTROL),
            Some(ZclValue::Bool(true))
        )
    }

    /// Get max current in centi-amps.
    pub fn current_max(&self) -> u16 {
        match self.store.get(ATTR_CURRENT_MAX) {
            Some(ZclValue::U16(v)) => *v,
            _ => 1600,
        }
    }

    /// Get max power in watts.
    pub fn power_max(&self) -> u16 {
        match self.store.get(ATTR_POWER_MAX) {
            Some(ZclValue::U16(v)) => *v,
            _ => 3680,
        }
    }

    /// Get auto-restart delay in seconds.
    pub fn time_reload(&self) -> u16 {
        match self.store.get(ATTR_TIME_RELOAD) {
            Some(ZclValue::U16(v)) => *v,
            _ => 30,
        }
    }

    /// Check if overload protection is enabled.
    pub fn is_protect_enabled(&self) -> bool {
        matches!(
            self.store.get(ATTR_PROTECT_CONTROL),
            Some(ZclValue::Bool(true))
        )
    }

    /// Check if auto-restart after overload is enabled.
    pub fn is_auto_restart(&self) -> bool {
        matches!(
            self.store.get(ATTR_AUTO_RESTART),
            Some(ZclValue::Bool(true))
        )
    }
}

impl Cluster for TuyaPlugCluster {
    fn cluster_id(&self) -> ClusterId {
        TUYA_CLUSTER_ID
    }

    fn handle_command(
        &mut self,
        _cmd_id: CommandId,
        _payload: &[u8],
    ) -> Result<heapless::Vec<u8, 64>, ZclStatus> {
        // No cluster-specific commands — attributes are read/written via foundation commands.
        Err(ZclStatus::UnsupClusterCommand)
    }

    fn attributes(&self) -> &dyn AttributeStoreAccess {
        &self.store
    }

    fn attributes_mut(&mut self) -> &mut dyn AttributeStoreMutAccess {
        &mut self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clusters::Cluster;

    #[test]
    fn cluster_id_is_fc01() {
        let cluster = TuyaPlugCluster::new();
        assert_eq!(cluster.cluster_id(), ClusterId(0xFC01));
    }

    #[test]
    fn default_values() {
        let cluster = TuyaPlugCluster::new();
        assert!(!cluster.is_key_locked());
        assert!(cluster.is_led_enabled());
        assert_eq!(cluster.current_max(), 1600);
        assert_eq!(cluster.power_max(), 3680);
        assert_eq!(cluster.time_reload(), 30);
        assert!(cluster.is_protect_enabled());
        assert!(!cluster.is_auto_restart());
    }

    #[test]
    fn read_attributes() {
        let cluster = TuyaPlugCluster::new();
        let attrs = cluster.attributes();
        assert_eq!(
            attrs.get(ATTR_KEY_LOCK),
            Some(&ZclValue::Bool(false))
        );
        assert_eq!(
            attrs.get(ATTR_LED_CONTROL),
            Some(&ZclValue::Bool(true))
        );
        assert_eq!(attrs.get(ATTR_CURRENT_MAX), Some(&ZclValue::U16(1600)));
        assert_eq!(attrs.get(ATTR_POWER_MAX), Some(&ZclValue::U16(3680)));
        assert_eq!(attrs.get(ATTR_TIME_RELOAD), Some(&ZclValue::U16(30)));
        assert_eq!(
            attrs.get(ATTR_PROTECT_CONTROL),
            Some(&ZclValue::Bool(true))
        );
        assert_eq!(
            attrs.get(ATTR_AUTO_RESTART),
            Some(&ZclValue::Bool(false))
        );
    }

    #[test]
    fn write_attributes() {
        let mut cluster = TuyaPlugCluster::new();
        let attrs = cluster.attributes_mut();

        attrs.set(ATTR_KEY_LOCK, ZclValue::Bool(true)).unwrap();
        attrs.set(ATTR_CURRENT_MAX, ZclValue::U16(1000)).unwrap();
        attrs.set(ATTR_POWER_MAX, ZclValue::U16(2300)).unwrap();
        attrs.set(ATTR_TIME_RELOAD, ZclValue::U16(60)).unwrap();
        attrs
            .set(ATTR_PROTECT_CONTROL, ZclValue::Bool(false))
            .unwrap();
        attrs
            .set(ATTR_AUTO_RESTART, ZclValue::Bool(true))
            .unwrap();

        assert!(cluster.is_key_locked());
        assert_eq!(cluster.current_max(), 1000);
        assert_eq!(cluster.power_max(), 2300);
        assert_eq!(cluster.time_reload(), 60);
        assert!(!cluster.is_protect_enabled());
        assert!(cluster.is_auto_restart());
    }

    #[test]
    fn handle_command_returns_unsupported() {
        let mut cluster = TuyaPlugCluster::new();
        let result = cluster.handle_command(CommandId(0x00), &[]);
        assert_eq!(result, Err(ZclStatus::UnsupClusterCommand));
    }

    #[test]
    fn all_attribute_ids() {
        let cluster = TuyaPlugCluster::new();
        let ids = cluster.attributes().all_ids();
        assert_eq!(ids.len(), 7);
        assert!(ids.contains(&ATTR_KEY_LOCK));
        assert!(ids.contains(&ATTR_LED_CONTROL));
        assert!(ids.contains(&ATTR_CURRENT_MAX));
        assert!(ids.contains(&ATTR_POWER_MAX));
        assert!(ids.contains(&ATTR_TIME_RELOAD));
        assert!(ids.contains(&ATTR_PROTECT_CONTROL));
        assert!(ids.contains(&ATTR_AUTO_RESTART));
    }
}
