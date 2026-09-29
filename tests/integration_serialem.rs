#![cfg(windows)]

use serialem_client::SerialEmClient;

#[test]
#[ignore = "requires a running SerialEM instance configured for external script control"]
fn connects_to_live_serialem_and_checks_external_control() {
    let mut client = SerialEmClient::connect_default().expect("connect to SerialEM");
    let ready = client
        .ok_to_run_external_script()
        .expect("query SerialEM external-script readiness");
    assert!(
        ready,
        "SerialEM is connected but not ready for an external script"
    );
}
