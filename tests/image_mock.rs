use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use serialem_client::Error;
use serialem_client::client::SerialEmClient;
use serialem_client::protocol::{
    MrcMode, PSS_CHUNK_HANDSHAKE, PSS_GET_BUFFER_IMAGE, PSS_PUT_IMAGE_IN_BUFFER,
    PYTHONMODULE_CHUNK_SIZE, decode_frame, encode_frame,
};
use serialem_client::transport::TransportConfig;

fn read_frame(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
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
fn receives_image_chunks_and_sends_the_protocol_handshake_between_chunks() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_frame(&mut stream);
        let decoded = decode_frame(&request, 3, 0, 0).unwrap();
        assert_eq!(decoded.longs, vec![PSS_GET_BUFFER_IMAGE, 0, 0]);

        let metadata = encode_frame(&[0, 0, 5, 5, 1, 5, 2], &[], &[], &[]).unwrap();
        stream.write_all(&metadata).unwrap();
        stream.write_all(b"abc").unwrap();

        let handshake = read_frame(&mut stream);
        let decoded = decode_frame(&handshake, 1, 0, 0).unwrap();
        assert_eq!(decoded.longs, vec![PSS_CHUNK_HANDSHAKE]);
        stream.write_all(b"de").unwrap();
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    let image = client
        .get_buffer_image(serialem_client::BufferIndex::new(0).unwrap(), false)
        .unwrap();
    assert_eq!(image.meta.mode, MrcMode::Byte);
    assert_eq!(image.bytes, b"abcde");
    worker.join().unwrap();
}

#[test]
fn compact_negative_put_image_ack_invalidates_connection() {
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
    let error = client
        .put_image_in_buffer(
            b"x",
            serialem_client::PutImageOptions {
                mode: MrcMode::Byte,
                size_x: 1,
                size_y: 1,
                to_buffer: serialem_client::BufferIndex::new(0).unwrap(),
                base_buffer: serialem_client::BufferIndex::new(0).unwrap(),
                more_binning: 1,
                capture_flag: -1,
            },
        )
        .unwrap_err();
    assert!(matches!(error, Error::SerialEm { code: -4 }));
    assert!(client.needs_reconnect());
    worker.join().unwrap();
}

#[test]
fn sends_image_metadata_then_raw_bytes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_frame(&mut stream);
        let decoded = decode_frame(&request, 10, 0, 0).unwrap();
        assert_eq!(
            decoded.longs,
            vec![PSS_PUT_IMAGE_IN_BUFFER, 0, 2, 2, 4, 0, 0, 1, -1, 1]
        );
        stream
            .write_all(&encode_frame(&[0], &[], &[], &[]).unwrap())
            .unwrap();

        let mut image = [0u8; 4];
        stream.read_exact(&mut image).unwrap();
        assert_eq!(&image, b"wxyz");
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    client
        .put_image_in_buffer(
            b"wxyz",
            serialem_client::PutImageOptions {
                mode: MrcMode::Byte,
                size_x: 2,
                size_y: 2,
                to_buffer: serialem_client::BufferIndex::new(0).unwrap(),
                base_buffer: serialem_client::BufferIndex::new(0).unwrap(),
                more_binning: 1,
                capture_flag: -1,
            },
        )
        .unwrap();
    worker.join().unwrap();
}

#[test]
fn sends_images_larger_than_one_pythonmodule_chunk() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let byte_count = PYTHONMODULE_CHUNK_SIZE + 1;
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_frame(&mut stream);
        let decoded = decode_frame(&request, 10, 0, 0).unwrap();
        assert_eq!(decoded.longs[4], byte_count as i32);
        stream
            .write_all(&encode_frame(&[0], &[], &[], &[]).unwrap())
            .unwrap();
        let mut received = vec![0u8; byte_count];
        stream.read_exact(&mut received).unwrap();
        assert_eq!(received[0], 0x5a);
        assert_eq!(received[byte_count - 1], 0xa5);
    });

    let mut image = vec![0x5au8; byte_count];
    image[byte_count - 1] = 0xa5;
    let mut client = SerialEmClient::connect(address).unwrap();
    client
        .put_image_in_buffer(
            &image,
            serialem_client::PutImageOptions {
                mode: MrcMode::Byte,
                size_x: byte_count,
                size_y: 1,
                to_buffer: serialem_client::BufferIndex::new(0).unwrap(),
                base_buffer: serialem_client::BufferIndex::new(0).unwrap(),
                more_binning: 1,
                capture_flag: -1,
            },
        )
        .unwrap();
    worker.join().unwrap();
}

#[test]
fn large_image_with_small_chunks_does_not_apply_image_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _request = read_frame(&mut stream);
        let metadata = encode_frame(&[0, 0, 2048, 2048, 1, 2048, 4], &[], &[], &[]).unwrap();
        stream.write_all(&metadata).unwrap();
        for index in 0..4 {
            if index > 0 {
                let _handshake = read_frame(&mut stream);
            }
            thread::sleep(Duration::from_millis(20));
            stream.write_all(&vec![b'a' + index as u8; 512]).unwrap();
        }
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    client.set_buffer_image_timeout(0.01).unwrap();
    let image = client.buffer_image("A").unwrap();
    assert_eq!(image.bytes.len(), 2048);
    worker.join().unwrap();
}

#[test]
fn restores_normal_timeout_for_a_small_final_chunk() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _request = read_frame(&mut stream);
        let metadata = encode_frame(&[0, 0, 2049, 1, 1, 2049, 2], &[], &[], &[]).unwrap();
        stream.write_all(&metadata).unwrap();
        stream.write_all(&vec![b'x'; 1025]).unwrap();
        let _handshake = read_frame(&mut stream);
        thread::sleep(Duration::from_millis(50));
        stream.write_all(&vec![b'y'; 1024]).unwrap();
    });

    let config = TransportConfig {
        read_timeout: Some(Duration::from_millis(200)),
        ..TransportConfig::default()
    };
    let mut client = SerialEmClient::connect_with_config(address, config).unwrap();
    client.set_buffer_image_timeout(0.01).unwrap();
    let image = client.buffer_image("A").unwrap();
    assert_eq!(image.bytes.len(), 2049);
    assert_eq!(image.bytes[0], b'x');
    assert_eq!(image.bytes[2048], b'y');
    worker.join().unwrap();
}

#[test]
fn applies_buffer_image_timeout_to_raw_chunk_reads() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _request = read_frame(&mut stream);
        let metadata = encode_frame(&[0, 0, 1025, 1025, 1, 1025, 1], &[], &[], &[]).unwrap();
        stream.write_all(&metadata).unwrap();
        thread::sleep(Duration::from_millis(200));
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    client.set_buffer_image_timeout(0.05).unwrap();
    let error = client.buffer_image("A").unwrap_err();
    assert!(matches!(
        error,
        Error::Io(ref io_error)
            if matches!(io_error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock)
    ));
    assert!(client.needs_reconnect());
    worker.join().unwrap();
}

#[test]
fn partial_image_eof_invalidates_the_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _request = read_frame(&mut stream);
        let metadata = encode_frame(&[0, 0, 5, 5, 1, 5, 1], &[], &[], &[]).unwrap();
        stream.write_all(&metadata).unwrap();
        stream.write_all(b"a").unwrap();
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    let error = client.buffer_image("A").unwrap_err();
    assert!(matches!(
        error,
        Error::Io(ref io_error) if io_error.kind() == std::io::ErrorKind::UnexpectedEof
    ));
    assert!(client.needs_reconnect());
    worker.join().unwrap();
}

#[test]
fn rejects_an_image_above_the_configured_limit_and_invalidates_the_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _request = read_frame(&mut stream);
        let metadata = encode_frame(&[0, 0, 5, 5, 1, 5, 1], &[], &[], &[]).unwrap();
        stream.write_all(&metadata).unwrap();
    });

    let config = TransportConfig {
        max_image_bytes: 4,
        ..TransportConfig::default()
    };
    let mut client = SerialEmClient::connect_with_config(address, config).unwrap();
    let error = client.buffer_image("A").unwrap_err();
    assert!(matches!(error, Error::ResourceLimit(_)));
    assert!(client.needs_reconnect());
    worker.join().unwrap();
}

#[test]
fn small_image_does_not_use_buffer_image_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _request = read_frame(&mut stream);
        let metadata = encode_frame(&[0, 0, 1, 1, 1, 1, 1], &[], &[], &[]).unwrap();
        stream.write_all(&metadata).unwrap();
        thread::sleep(Duration::from_millis(100));
        stream.write_all(b"a").unwrap();
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    client.set_buffer_image_timeout(0.01).unwrap();
    let image = client.buffer_image("A").unwrap();
    assert_eq!(image.bytes, b"a");
    worker.join().unwrap();
}

#[test]
fn restores_normal_timeout_after_large_image_transfer() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _large_request = read_frame(&mut stream);
        let large_metadata = encode_frame(&[0, 0, 1025, 1025, 1, 1025, 1], &[], &[], &[]).unwrap();
        stream.write_all(&large_metadata).unwrap();
        stream.write_all(&vec![b'x'; 1025]).unwrap();

        let _small_request = read_frame(&mut stream);
        let small_metadata = encode_frame(&[0, 0, 1, 1, 1, 1, 1], &[], &[], &[]).unwrap();
        stream.write_all(&small_metadata).unwrap();
        thread::sleep(Duration::from_millis(100));
        stream.write_all(b"a").unwrap();
    });

    let mut client = SerialEmClient::connect(address).unwrap();
    client.set_buffer_image_timeout(0.01).unwrap();
    let large = client.buffer_image("A").unwrap();
    assert_eq!(large.bytes.len(), 1025);
    let small = client.buffer_image("A").unwrap();
    assert_eq!(small.bytes, b"a");
    worker.join().unwrap();
}
