#![cfg(not(feature = "router"))]

use core::future::Future;
use core::task::{Context, Poll, Waker};

use zigbee_aps::ApsLayer;
use zigbee_bdb::{BdbLayer, BdbStatus};
use zigbee_mac::mock::MockMac;
use zigbee_nwk::frames::NwkHeader;
use zigbee_nwk::{DeviceType, NwkLayer};
use zigbee_types::{PanId, ShortAddress};
use zigbee_zdo::ZdoLayer;

fn block_on<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);

    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::yield_now();
    }
}

fn joined_end_device() -> BdbLayer<MockMac> {
    let ieee = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    let mac = MockMac::new(ieee);
    let mut nwk = NwkLayer::new(mac, DeviceType::EndDevice);
    nwk.set_joined(true);
    nwk.security_mut().set_network_key([0x5A; 16], 0);
    {
        let nib = nwk.nib_mut();
        nib.network_address = ShortAddress(0x1234);
        nib.pan_id = PanId(0x5678);
        nib.ieee_address = ieee;
        nib.logical_channel = 15;
        nib.security_enabled = true;
        nib.outgoing_frame_counter_limit = 0x400;
    }
    let aps = ApsLayer::new(nwk);
    let mut zdo = ZdoLayer::new(aps);
    zdo.set_local_nwk_addr(ShortAddress(0x1234));
    zdo.set_local_ieee_addr(ieee);
    let mut bdb = BdbLayer::new(zdo);
    bdb.attributes_mut().node_is_on_a_network = true;
    bdb
}

/// BDB v3.0.1 §8.2: an end device on a network still performs network
/// steering by broadcasting Mgmt_Permit_Joining_req; only the local
/// NLME-PERMIT-JOINING step is router/coordinator behaviour. This build has
/// no `router` feature, so it exercises the end-device-only code path.
#[test]
fn no_router_end_device_broadcasts_mgmt_permit_joining() {
    let mut bdb = joined_end_device();

    assert_eq!(block_on(bdb.network_steering()), Ok(()));

    let history = bdb.zdo().nwk().mac().tx_history();
    assert_eq!(history.len(), 1);
    let (header, _) = NwkHeader::parse(history[0].payload.as_slice()).unwrap();
    assert_eq!(
        header.dst_addr,
        ShortAddress::BROADCAST_ROUTERS_AND_COORDINATOR
    );
    assert!(!bdb.zdo().nwk().nib().permit_joining);
}

#[cfg(feature = "end-device")]
#[test]
fn end_device_formation_is_explicitly_rejected() {
    let mac = MockMac::new([0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
    let nwk = NwkLayer::new(mac, DeviceType::EndDevice);
    let aps = ApsLayer::new(nwk);
    let zdo = ZdoLayer::new(aps);
    let mut bdb = BdbLayer::new(zdo);

    assert_eq!(
        block_on(bdb.network_formation()),
        Err(BdbStatus::NotPermitted)
    );
}
