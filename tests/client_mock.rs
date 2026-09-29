use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use serialem_client::client::SerialEmClient;
use serialem_client::protocol::{
    PSS_OK_TO_RUN_EXTERNAL_SCRIPT, PSS_REGULAR_COMMAND, decode_frame, encode_frame,
};

fn read_frame(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).unwrap();
    let length = u32::from_le_bytes(header) as usize;
    let mut frame = vec![0u8; length];
    frame[..4].copy_from_slice(&header);
    stream.read_exact(&mut frame[4..]).unwrap();
    frame
}

#[test]
fn compact_negative_handshake_error_maps_to_busy_and_invalidates() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _request = read_frame(&mut stream);
        stream
            .write_all(&encode_frame(&[-9], &[], &[], &[]).unwrap())
            .unwrap();
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    assert!(matches!(
        client.ok_to_run_external_script(),
        Err(serialem_client::Error::Busy)
    ));
    assert!(client.needs_reconnect());
    worker.join().unwrap();
}

#[test]
fn compact_negative_regular_error_maps_to_user_stop_and_invalidates() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _handshake = read_frame(&mut stream);
        stream
            .write_all(&encode_frame(&[0], &[1], &[], &[]).unwrap())
            .unwrap();
        let _command = read_frame(&mut stream);
        stream
            .write_all(&encode_frame(&[-10], &[], &[], &[]).unwrap())
            .unwrap();
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    assert!(matches!(
        client.execute_regular(123, &[], 0),
        Err(serialem_client::Error::UserStop)
    ));
    assert!(client.needs_reconnect());
    worker.join().unwrap();
}

#[test]
fn compact_negative_image_metadata_error_maps_to_serialem_and_invalidates() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _request = read_frame(&mut stream);
        stream
            .write_all(&encode_frame(&[-4], &[], &[], &[]).unwrap())
            .unwrap();
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    assert!(matches!(
        client.buffer_image("A"),
        Err(serialem_client::Error::SerialEm { code: -4 })
    ));
    assert!(client.needs_reconnect());
    worker.join().unwrap();
}

#[test]
fn client_performs_external_control_handshake_before_regular_command() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();

        let handshake = read_frame(&mut stream);
        let decoded = decode_frame(&handshake, 1, 0, 0).unwrap();
        assert_eq!(decoded.longs, vec![PSS_OK_TO_RUN_EXTERNAL_SCRIPT]);
        stream
            .write_all(&encode_frame(&[0], &[1], &[], &[]).unwrap())
            .unwrap();

        let command = read_frame(&mut stream);
        let decoded = decode_frame(&command, 4, 0, 0).unwrap();
        assert_eq!(decoded.longs, vec![PSS_REGULAR_COMMAND, 123, 0, 1]);
        assert_eq!(decoded.payload, vec![0, 0, 0, 0]);
        stream
            .write_all(&encode_frame(&[0, -1, 0, 0], &[], &[], &[]).unwrap())
            .unwrap();
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    let response = client.execute_regular(123, &[], 0).unwrap();
    assert_eq!(response.return_code, 0);
    assert!(response.reports.is_empty());
    worker.join().unwrap();
}

#[test]
fn reconnect_after_server_execution_repeats_handshake_before_next_command() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        let handshake = read_frame(&mut first);
        assert_eq!(
            decode_frame(&handshake, 1, 0, 0).unwrap().longs,
            vec![PSS_OK_TO_RUN_EXTERNAL_SCRIPT]
        );
        first
            .write_all(&encode_frame(&[0], &[1], &[], &[]).unwrap())
            .unwrap();
        let command = read_frame(&mut first);
        assert_eq!(
            decode_frame(&command, 4, 0, 0).unwrap().longs,
            vec![PSS_REGULAR_COMMAND, 321, 0, 1]
        );
        // The server may already have executed the command before the response was lost.
        drop(first);

        let (mut second, _) = listener.accept().unwrap();
        let handshake = read_frame(&mut second);
        assert_eq!(
            decode_frame(&handshake, 1, 0, 0).unwrap().longs,
            vec![PSS_OK_TO_RUN_EXTERNAL_SCRIPT]
        );
        second
            .write_all(&encode_frame(&[0], &[1], &[], &[]).unwrap())
            .unwrap();
        let command = read_frame(&mut second);
        assert_eq!(
            decode_frame(&command, 4, 0, 0).unwrap().longs,
            vec![PSS_REGULAR_COMMAND, 321, 0, 1]
        );
        second
            .write_all(&encode_frame(&[0, -1, 0, 0], &[], &[], &[]).unwrap())
            .unwrap();
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    assert!(client.execute_regular(321, &[], 0).is_err());
    assert!(client.needs_reconnect());
    let response = client.execute_regular(321, &[], 0).unwrap();
    assert_eq!(response.return_code, 0);
    worker.join().unwrap();
}

#[test]
fn invalid_regular_arguments_fail_before_external_control_handshake() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(50)))
            .unwrap();
        let mut bytes = [0u8; 4];
        let result = stream.read(&mut bytes);
        assert!(matches!(result, Err(error) if error.kind() == std::io::ErrorKind::TimedOut));
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    let error = client.execute_regular(777, &[], usize::MAX).unwrap_err();
    assert!(matches!(error, serialem_client::Error::InvalidArgument(_)));
    worker.join().unwrap();
}

#[test]
fn a_false_readiness_result_clears_initialized_state() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let first = read_frame(&mut stream);
        assert_eq!(
            decode_frame(&first, 1, 0, 0).unwrap().longs,
            vec![PSS_OK_TO_RUN_EXTERNAL_SCRIPT]
        );
        stream
            .write_all(&encode_frame(&[0], &[0], &[], &[]).unwrap())
            .unwrap();

        let second = read_frame(&mut stream);
        assert_eq!(
            decode_frame(&second, 1, 0, 0).unwrap().longs,
            vec![PSS_OK_TO_RUN_EXTERNAL_SCRIPT]
        );
        stream
            .write_all(&encode_frame(&[0], &[1], &[], &[]).unwrap())
            .unwrap();
        let command = read_frame(&mut stream);
        assert_eq!(
            decode_frame(&command, 4, 0, 0).unwrap().longs,
            vec![PSS_REGULAR_COMMAND, 777, 0, 1]
        );
        stream
            .write_all(&encode_frame(&[0, -1, 0, 0], &[], &[], &[]).unwrap())
            .unwrap();
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    client.mark_script_initialized();
    assert!(!client.ok_to_run_external_script().unwrap());
    assert_eq!(client.execute_regular(777, &[], 0).unwrap().return_code, 0);
    worker.join().unwrap();
}

#[test]
fn malformed_regular_response_invalidates_the_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _handshake = read_frame(&mut stream);
        stream
            .write_all(&encode_frame(&[0], &[1], &[], &[]).unwrap())
            .unwrap();
        let _command = read_frame(&mut stream);
        let malformed = encode_frame(&[0, -1, 0, 25], &[], &[], &[]).unwrap();
        stream.write_all(&malformed).unwrap();
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    let error = client.execute_regular(123, &[], 0).unwrap_err();
    assert!(matches!(error, serialem_client::Error::InvalidFrame(_)));
    assert!(client.needs_reconnect());
    worker.join().unwrap();
}
