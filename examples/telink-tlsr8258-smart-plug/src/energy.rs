//! Energy persistence via NvStorage.

use zigbee_runtime::nv_storage::{NvError, NvItemId, NvStorage};
use zigbee_runtime::profile::smart_plug::SmartPlug;

/// Persist energy counter to NvStorage every `INTERVAL_SECS` seconds.
pub const PERSIST_INTERVAL_SECS: u32 = 60;

/// Load saved energy from NvStorage and apply to SmartPlug.
pub fn load_energy<N: NvStorage>(storage: &mut N, plug: &mut SmartPlug) {
    let mut buf = [0u8; 8];
    match storage.read(NvItemId::AppEnergyWh, &mut buf) {
        Ok(8) => {
            let wh = u64::from_le_bytes(buf);
            plug.add_energy_delivered_wh(wh);
        }
        Ok(_) => {} // Corrupted, start from 0
        Err(NvError::NotFound) => {} // First boot, start from 0
        Err(_) => {}                  // Storage error, start from 0
    }
}

/// Save current energy counter to NvStorage.
pub fn save_energy<N: NvStorage>(storage: &mut N, plug: &SmartPlug) {
    if let Some(wh) = plug.total_energy_delivered_wh() {
        let buf = wh.to_le_bytes();
        let _ = storage.write(NvItemId::AppEnergyWh, &buf);
    }
}
