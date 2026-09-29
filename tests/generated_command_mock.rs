use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use serialem_client::Error;
use serialem_client::client::SerialEmClient;
use serialem_client::generated::command_ids::CME_SETMAG;
use serialem_client::protocol::{
    PSS_OK_TO_RUN_EXTERNAL_SCRIPT, PSS_REGULAR_COMMAND, decode_frame, encode_frame,
};

fn read_frame(stream: &mut TcpStream) -> Vec<u8> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).unwrap();
    let length = u32::from_le_bytes(header) as usize;
    let mut frame = vec![0u8; length];
    frame[..4].copy_from_slice(&header);
    stream.read_exact(&mut frame[4..]).unwrap();
    frame
}

#[test]
fn generated_set_mag_wrapper_uses_the_pinned_command_id() {
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
        assert_eq!(decoded.longs[0], PSS_REGULAR_COMMAND);
        assert_eq!(decoded.longs[1], CME_SETMAG);
        assert_eq!(decoded.longs[2], 1);
        assert_eq!(decoded.longs[3], 9);
        assert_eq!(decoded.payload.len(), 36);
        stream
            .write_all(&encode_frame(&[0, -1, 0, 0], &[], &[], &[]).unwrap())
            .unwrap();
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    assert!(matches!(
        client.TiltUp(None, Some(1.0), None),
        Err(Error::InvalidArgument(message)) if message.contains("optional argument")
    ));
    assert_eq!(
        client.SetMag(20_000.0, None).unwrap(),
        serialem_client::CommandResult::None
    );
    worker.join().unwrap();
}
