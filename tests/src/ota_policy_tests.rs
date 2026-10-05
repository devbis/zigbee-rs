//! OTA manager policy tests: explicit acceptance, image validation, and
//! hostile-image handling (runtime `OtaManager`).

use zigbee_runtime::event_loop::StackEvent;
use zigbee_runtime::firmware_writer::MockFirmwareWriter;
use zigbee_runtime::ota::{OtaConfig, OtaImageOffer, OtaManager};
use zigbee_zcl::clusters::ota::*;

const MFG: u16 = 0x1234;
const IMG: u16 = 0x0001;
const CUR: u32 = 1;
const NEW: u32 = 2;

fn manager(auto_accept: bool) -> OtaManager<MockFirmwareWriter> {
    OtaManager::new(
        MockFirmwareWriter::new(4096),
        OtaConfig {
            manufacturer_code: MFG,
            image_type: IMG,
            current_version: CUR,
            endpoint: 1,
            block_size: 48,
            auto_accept,
            hardware_version: None,
        },
    )
}

/// `[header(56)] [tag(2) len(4) body]...` with a consistent total size.
fn ota_file(version: u32, elements: &[(u16, &[u8])]) -> Vec<u8> {
    let total: u32 = 56
        + elements
            .iter()
            .map(|(_, b)| 6 + b.len() as u32)
            .sum::<u32>();
    let mut f = Vec::new();
    f.extend_from_slice(&0x0BEE_F11Eu32.to_le_bytes());
    f.extend_from_slice(&0x0100u16.to_le_bytes());
    f.extend_from_slice(&56u16.to_le_bytes());
    f.extend_from_slice(&0u16.to_le_bytes());
    f.extend_from_slice(&MFG.to_le_bytes());
    f.extend_from_slice(&IMG.to_le_bytes());
    f.extend_from_slice(&version.to_le_bytes());
    f.extend_from_slice(&0x0002u16.to_le_bytes());
    f.extend_from_slice(&[0u8; 32]);
    f.extend_from_slice(&total.to_le_bytes());
    for (tag, body) in elements {
        f.extend_from_slice(&tag.to_le_bytes());
        f.extend_from_slice(&(body.len() as u32).to_le_bytes());
        f.extend_from_slice(body);
    }
    assert_eq!(f.len() as u32, total);
    f
}

fn query_response(version: u32, size: u32) -> [u8; 13] {
    let mut r = [0u8; 13];
    r[1..3].copy_from_slice(&MFG.to_le_bytes());
    r[3..5].copy_from_slice(&IMG.to_le_bytes());
    r[5..9].copy_from_slice(&version.to_le_bytes());
    r[9..13].copy_from_slice(&size.to_le_bytes());
    r
}

fn block(version: u32, offset: u32, chunk: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 14];
    b[1..3].copy_from_slice(&MFG.to_le_bytes());
    b[3..5].copy_from_slice(&IMG.to_le_bytes());
    b[5..9].copy_from_slice(&version.to_le_bytes());
    b[9..13].copy_from_slice(&offset.to_le_bytes());
    b[13] = chunk.len() as u8;
    b.extend_from_slice(chunk);
    b
}

/// Feed the whole file; returns the last event (progress, or the failure).
fn feed(mgr: &mut OtaManager<MockFirmwareWriter>, version: u32, file: &[u8]) -> Option<StackEvent> {
    let mut offset = 0usize;
    let mut last = None;
    while offset < file.len() {
        let end = (offset + 48).min(file.len());
        last = mgr.handle_incoming(
            CMD_IMAGE_BLOCK_RESPONSE.0,
            &block(version, offset as u32, &file[offset..end]),
            None,
        );
        if matches!(last, Some(StackEvent::OtaFailed)) {
            return last;
        }
        offset = end;
    }
    last
}

fn offer(version: u32, size: u32) -> OtaImageOffer {
    OtaImageOffer {
        manufacturer_code: MFG,
        image_type: IMG,
        file_version: version,
        image_size: size,
    }
}

/// Start a download with auto-accept and return the first-block event.
fn start_auto(mgr: &mut OtaManager<MockFirmwareWriter>, version: u32, size: u32) {
    mgr.start_query();
    let _ = mgr.take_pending_frame();
    let ev = mgr.handle_incoming(
        CMD_QUERY_NEXT_IMAGE_RESPONSE.0,
        &query_response(version, size),
        None,
    );
    assert!(matches!(ev, Some(StackEvent::OtaImageAvailable { .. })));
    let _ = mgr.take_pending_frame();
}

fn assert_failed_with_invalid_image(mgr: &mut OtaManager<MockFirmwareWriter>) {
    let frame = mgr
        .take_pending_frame()
        .expect("failure must queue an Upgrade End Request");
    assert_eq!(frame.zcl_data[2], CMD_UPGRADE_END_REQUEST.0);
    assert_eq!(frame.zcl_data[3], 0x96, "status must be INVALID_IMAGE");
    assert_ne!(mgr.state(), OtaState::WaitingActivate);
}

#[test]
fn explicit_acceptance_downloads_the_accepted_image_without_looping() {
    let mut mgr = manager(false);
    let file = ota_file(NEW, &[(0x0000, &[0x11; 70])]);
    let size = file.len() as u32;

    mgr.start_query();
    let _ = mgr.take_pending_frame();
    let ev = mgr.handle_incoming(
        CMD_QUERY_NEXT_IMAGE_RESPONSE.0,
        &query_response(NEW, size),
        None,
    );
    assert!(matches!(
        ev,
        Some(StackEvent::OtaImageAvailable { version: NEW, size: s }) if s == size
    ));
    assert_eq!(
        mgr.state(),
        OtaState::Idle,
        "download paused until accepted"
    );
    assert!(
        mgr.take_pending_frame().is_none(),
        "no block request before acceptance"
    );
    assert_eq!(mgr.offered_image(), Some(offer(NEW, size)));
    assert_eq!(mgr.writer().bytes_written(), 0);

    assert!(mgr.accept_ota().is_none());
    assert_eq!(mgr.accepted_image(), Some(offer(NEW, size)));
    assert_eq!(mgr.offered_image(), None);
    let query = mgr
        .take_pending_frame()
        .expect("acceptance re-queries the server");
    assert_eq!(query.zcl_data[2], CMD_QUERY_NEXT_IMAGE_REQUEST.0);

    // Same image offered again: the download starts silently (no new event).
    let ev = mgr.handle_incoming(
        CMD_QUERY_NEXT_IMAGE_RESPONSE.0,
        &query_response(NEW, size),
        None,
    );
    assert!(
        ev.is_none(),
        "accepted image must not be re-announced: {ev:?}"
    );
    assert!(matches!(mgr.state(), OtaState::Downloading { .. }));
    let first = mgr.take_pending_frame().expect("block request queued");
    assert_eq!(first.zcl_data[2], CMD_IMAGE_BLOCK_REQUEST.0);

    let last = feed(&mut mgr, NEW, &file);
    assert!(
        matches!(last, Some(StackEvent::OtaProgress { .. })),
        "{last:?}"
    );
    assert_eq!(mgr.state(), OtaState::WaitingActivate);
    assert_eq!(mgr.writer().data(), &[0x11; 70][..]);
    assert_eq!(mgr.accepted_image(), None, "acceptance is single-use");
}

#[test]
fn explicit_acceptance_does_not_cover_a_different_image() {
    let mut mgr = manager(false);
    mgr.start_query();
    let _ = mgr.take_pending_frame();
    let _ = mgr.handle_incoming(
        CMD_QUERY_NEXT_IMAGE_RESPONSE.0,
        &query_response(NEW, 200),
        None,
    );
    assert!(mgr.accept_ota().is_none());
    let _ = mgr.take_pending_frame();

    // The server now offers a newer image: the application must decide again.
    let ev = mgr.handle_incoming(
        CMD_QUERY_NEXT_IMAGE_RESPONSE.0,
        &query_response(3, 200),
        None,
    );
    assert!(matches!(
        ev,
        Some(StackEvent::OtaImageAvailable { version: 3, .. })
    ));
    assert_eq!(mgr.state(), OtaState::Idle);
    assert_eq!(mgr.accepted_image(), None);
    assert_eq!(mgr.offered_image(), Some(offer(3, 200)));
    assert!(mgr.take_pending_frame().is_none());
}

#[test]
fn accept_without_offer_is_a_no_op() {
    let mut mgr = manager(false);
    assert!(mgr.accept_ota().is_none());
    assert!(mgr.take_pending_frame().is_none());
    assert_eq!(mgr.state(), OtaState::Idle);
}

#[test]
fn auto_accept_announces_and_downloads_immediately() {
    let mut mgr = manager(true);
    let file = ota_file(NEW, &[(0x0000, &[0x22; 10])]);
    start_auto(&mut mgr, NEW, file.len() as u32);
    assert!(matches!(mgr.state(), OtaState::Downloading { .. }));
    let last = feed(&mut mgr, NEW, &file);
    assert!(matches!(last, Some(StackEvent::OtaProgress { .. })));
    assert_eq!(mgr.state(), OtaState::WaitingActivate);
}

#[test]
fn header_version_mismatch_is_rejected() {
    let mut mgr = manager(true);
    // Server advertises v2, but the image header carries v3.
    let file = ota_file(3, &[(0x0000, &[0x33; 40])]);
    start_auto(&mut mgr, NEW, file.len() as u32);
    assert!(matches!(
        feed(&mut mgr, NEW, &file),
        Some(StackEvent::OtaFailed)
    ));
    assert_failed_with_invalid_image(&mut mgr);
}

#[test]
fn header_total_size_mismatch_is_rejected() {
    let mut mgr = manager(true);
    let mut file = ota_file(NEW, &[(0x0000, &[0x44; 40])]);
    let bogus = (file.len() as u32 + 8).to_le_bytes();
    file[52..56].copy_from_slice(&bogus);
    start_auto(&mut mgr, NEW, file.len() as u32);
    assert!(matches!(
        feed(&mut mgr, NEW, &file),
        Some(StackEvent::OtaFailed)
    ));
    assert_failed_with_invalid_image(&mut mgr);
}

/// `ota_file` with a hardware-version range in the header (field control
/// bit 2, ZCL r8 §11.4.2): the header grows to 60 bytes.
fn ota_file_with_hardware_range(version: u32, min: u16, max: u16, body: &[u8]) -> Vec<u8> {
    let mut f = ota_file(version, &[(0x0000, body)]);
    let total = f.len() as u32 + 4;
    f[6..8].copy_from_slice(&60u16.to_le_bytes());
    f[8..10].copy_from_slice(&0x0004u16.to_le_bytes());
    f[52..56].copy_from_slice(&total.to_le_bytes());
    let mut range = Vec::new();
    range.extend_from_slice(&min.to_le_bytes());
    range.extend_from_slice(&max.to_le_bytes());
    f.splice(56..56, range);
    f
}

fn manager_with_hardware_version(hardware_version: u16) -> OtaManager<MockFirmwareWriter> {
    OtaManager::new(
        MockFirmwareWriter::new(4096),
        OtaConfig {
            manufacturer_code: MFG,
            image_type: IMG,
            current_version: CUR,
            endpoint: 1,
            block_size: 48,
            auto_accept: true,
            hardware_version: Some(hardware_version),
        },
    )
}

#[test]
fn header_hardware_range_excluding_this_device_is_rejected() {
    let mut mgr = manager_with_hardware_version(5);
    let file = ota_file_with_hardware_range(NEW, 1, 3, &[0x55; 40]);
    start_auto(&mut mgr, NEW, file.len() as u32);
    assert!(matches!(
        feed(&mut mgr, NEW, &file),
        Some(StackEvent::OtaFailed)
    ));
    assert_failed_with_invalid_image(&mut mgr);
}

#[test]
fn header_hardware_range_including_this_device_is_accepted() {
    let mut mgr = manager_with_hardware_version(5);
    let file = ota_file_with_hardware_range(NEW, 1, 9, &[0x66; 40]);
    start_auto(&mut mgr, NEW, file.len() as u32);
    feed(&mut mgr, NEW, &file);
    assert_eq!(mgr.state(), OtaState::WaitingActivate);
}

#[test]
fn image_without_upgrade_image_tag_fails_verification() {
    let mut mgr = manager(true);
    // Only a manufacturer-specific element; no tag 0x0000.
    let file = ota_file(NEW, &[(0xF000, &[0x55; 30])]);
    start_auto(&mut mgr, NEW, file.len() as u32);
    assert!(matches!(
        feed(&mut mgr, NEW, &file),
        Some(StackEvent::OtaFailed)
    ));
    assert_failed_with_invalid_image(&mut mgr);
    assert_eq!(mgr.writer().bytes_written(), 0, "nothing must be staged");
}

#[test]
fn empty_upgrade_image_tag_is_rejected() {
    let mut mgr = manager(true);
    let file = ota_file(NEW, &[(0x0000, &[])]);
    start_auto(&mut mgr, NEW, file.len() as u32);
    assert!(matches!(
        feed(&mut mgr, NEW, &file),
        Some(StackEvent::OtaFailed)
    ));
    assert_failed_with_invalid_image(&mut mgr);
}

#[test]
fn upgrade_image_after_other_elements_is_extracted() {
    let mut mgr = manager(true);
    let file = ota_file(NEW, &[(0x0001, &[0xEE; 20]), (0x0000, &[0x66; 33])]);
    start_auto(&mut mgr, NEW, file.len() as u32);
    assert!(matches!(
        feed(&mut mgr, NEW, &file),
        Some(StackEvent::OtaProgress { .. })
    ));
    assert_eq!(mgr.state(), OtaState::WaitingActivate);
    assert_eq!(mgr.writer().data(), &[0x66; 33][..]);
}

#[test]
fn hostile_sub_element_lengths_fail_without_wrapping() {
    for tag in [0x0000u16, 0x0001] {
        for len in [u32::MAX, u32::MAX - 5, 0x8000_0000] {
            let mut mgr = manager(true);
            let mut file = ota_file(NEW, &[(tag, &[0x77; 20])]);
            file[58..62].copy_from_slice(&len.to_le_bytes());
            start_auto(&mut mgr, NEW, file.len() as u32);
            assert!(
                matches!(feed(&mut mgr, NEW, &file), Some(StackEvent::OtaFailed)),
                "tag {tag:#x} len {len:#x} must fail"
            );
            assert_failed_with_invalid_image(&mut mgr);
        }
    }
}

#[test]
fn missing_upgrade_end_response_keeps_the_verified_image() {
    let mut mgr = manager(true);
    let file = ota_file(NEW, &[(0x0000, &[0x88; 50])]);
    start_auto(&mut mgr, NEW, file.len() as u32);
    feed(&mut mgr, NEW, &file);
    assert_eq!(mgr.state(), OtaState::WaitingActivate);
    let end = mgr.take_pending_frame().expect("end request");
    assert_eq!(end.zcl_data[2], CMD_UPGRADE_END_REQUEST.0);

    for _ in 0..3 {
        assert!(mgr.tick(120).is_none());
        let retry = mgr.take_pending_frame().expect("end request retransmitted");
        assert_eq!(retry.zcl_data[2], CMD_UPGRADE_END_REQUEST.0);
        assert_eq!(retry.zcl_data[3], 0x00);
        assert_eq!(mgr.state(), OtaState::WaitingActivate);
    }
    assert!(matches!(mgr.tick(120), Some(StackEvent::OtaFailed)));
    assert_eq!(mgr.state(), OtaState::Idle);
    assert_eq!(
        mgr.writer().data(),
        &[0x88; 50][..],
        "verified image not erased"
    );
    assert!(!mgr.writer().is_activated());
    assert!(
        mgr.activate().is_err(),
        "an unacknowledged image is never activated"
    );
}
