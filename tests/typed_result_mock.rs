use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use serialem_client::client::SerialEmClient;
use serialem_client::generated::command_ids::{CME_REPORTLENS, CME_REPORTMAG, CME_SETMAG};
use serialem_client::protocol::{
    PSS_OK_TO_RUN_EXTERNAL_SCRIPT, PSS_REGULAR_COMMAND, SCRIPT_USER_STOP, decode_frame,
    encode_frame,
};
use serialem_client::{CommandResult, Error, ReportValue};

fn read_frame(stream: &mut TcpStream) -> Vec<u8> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).unwrap();
    let length = u32::from_le_bytes(header) as usize;
    let mut frame = vec![0u8; length];
    frame[..4].copy_from_slice(&header);
    stream.read_exact(&mut frame[4..]).unwrap();
    frame
}

fn send_number(stream: &mut TcpStream, value: f64) {
    let mut payload = Vec::new();
    payload.extend_from_slice(&0i32.to_le_bytes());
    payload.extend_from_slice(&value.to_le_bytes());
    stream
        .write_all(&encode_frame(&[0, 0, 0, 3], &[], &[], &payload).unwrap())
        .unwrap();
}

fn send_text(stream: &mut TcpStream, value: &str) {
    let mut payload = Vec::new();
    payload.extend_from_slice(&1i32.to_le_bytes());
    payload.extend_from_slice(&0f64.to_le_bytes());
    payload.extend_from_slice(value.as_bytes());
    payload.push(0);
    payload.resize(28, 0);
    stream
        .write_all(&encode_frame(&[0, 0, 0, 7], &[], &[], &payload).unwrap())
        .unwrap();
}

#[test]
fn maps_numeric_string_tuple_and_stop_results() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let handshake = read_frame(&mut stream);
        assert_eq!(
            decode_frame(&handshake, 1, 0, 0).unwrap().longs,
            vec![PSS_OK_TO_RUN_EXTERNAL_SCRIPT]
        );
        stream
            .write_all(&encode_frame(&[0], &[1], &[], &[]).unwrap())
            .unwrap();

        let command = read_frame(&mut stream);
        let decoded = decode_frame(&command, 4, 0, 0).unwrap();
        assert_eq!(
            decoded.longs,
            vec![PSS_REGULAR_COMMAND, CME_REPORTMAG, 0, 1,]
        );
        assert_eq!(decoded.payload, vec![0, 0, 0, 0]);
        send_number(&mut stream, 123.5);

        let command = read_frame(&mut stream);
        let decoded = decode_frame(&command, 4, 0, 0).unwrap();
        assert_eq!(
            decoded.longs,
            vec![PSS_REGULAR_COMMAND, CME_REPORTMAG, 0, 1,]
        );
        assert_eq!(decoded.payload, vec![0, 0, 0, 0]);
        send_number(&mut stream, 124.5);

        let command = read_frame(&mut stream);
        let decoded = decode_frame(&command, 4, 0, 0).unwrap();
        assert_eq!(decoded.longs[0], PSS_REGULAR_COMMAND);
        assert_eq!(decoded.longs[1], CME_REPORTLENS);
        assert_eq!(decoded.longs[2], 1);
        send_text(&mut stream, "objective");

        let command = read_frame(&mut stream);
        assert_eq!(
            decode_frame(&command, 4, 0, 0).unwrap().longs[1],
            CME_SETMAG
        );
        stream
            .write_all(&encode_frame(&[0, -1, SCRIPT_USER_STOP, 0], &[], &[], &[]).unwrap())
            .unwrap();
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    assert_eq!(client.ReportMag().unwrap(), CommandResult::Number(123.5));
    client.return_all_values_as_tuples(true);
    assert_eq!(
        client.ReportMag().unwrap(),
        CommandResult::Tuple(vec![ReportValue::Number(124.5)])
    );
    assert_eq!(
        client.ReportLens("objective").unwrap(),
        CommandResult::Text("objective".into())
    );
    assert!(matches!(
        client.SetMag(20_000.0, None),
        Err(Error::ScriptExited("user STOP"))
    ));
    worker.join().unwrap();
}
