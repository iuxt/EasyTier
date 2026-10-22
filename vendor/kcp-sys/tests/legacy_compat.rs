use std::{sync::Arc, time::Duration};

use bytes::{Bytes, BytesMut};
use kcp_sys_legacy::{endpoint::KcpEndpoint, packet_def::KcpPacket};
use parking_lot::Mutex;
use tokio::{
    io::{AsyncReadExt, AsyncWrite, AsyncWriteExt},
    task::JoinSet,
    time::{timeout, Instant},
};

const HEADER_LEN: usize = 14;
const FLAGS_OFFSET: usize = 12;
const MODE_OFFSET: usize = 13;
const SYN: u8 = 0x01;
const ACK: u8 = 0x02;
const FIN: u8 = 0x04;
const RST: u8 = 0x10;
const FIN_ACK: u8 = 0x80;

fn control_packet(flags: u8, mode: u8) -> BytesMut {
    let mut packet = KcpPacket::new(0);
    packet
        .mut_header()
        .set_conv(123)
        .set_src_session_id(1)
        .set_dst_session_id(2);
    let mut bytes = packet.inner();
    assert_eq!(bytes.len(), HEADER_LEN);
    bytes[FLAGS_OFFSET] = flags;
    bytes[MODE_OFFSET] = mode;
    bytes
}

#[tokio::test]
async fn legacy_endpoint_ignores_mode_offer_and_suppresses_duplicate_syn_reset() {
    let mut endpoint = KcpEndpoint::new();
    endpoint.run().await;
    let sender = endpoint.input_sender();
    let mut receiver = endpoint.output_receiver().unwrap();
    let syn = control_packet(SYN, 1);
    sender.send(syn.clone().into()).await.unwrap();

    let syn_ack = timeout(Duration::from_secs(1), async {
        loop {
            let packet = receiver.recv().await.unwrap().inner();
            assert_eq!(packet[FLAGS_OFFSET] & RST, 0);
            if packet[FLAGS_OFFSET] == SYN | ACK {
                break packet;
            }
        }
    })
    .await
    .expect("the real legacy endpoint must accept a mode offer");
    assert_eq!(syn_ack[MODE_OFFSET], 0);

    sender.send(syn.into()).await.unwrap();
    let deadline = Instant::now() + Duration::from_millis(250);
    while let Ok(Some(packet)) = tokio::time::timeout_at(deadline, receiver.recv()).await {
        assert!(!packet.header().is_rst(), "duplicate SYN emitted RST");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FromEndpoint {
    Current,
    Legacy,
}

async fn write_chunks(stream: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) {
    // KCP limits the number of fragments in a single send operation.
    for chunk in bytes.chunks(16 * 1024) {
        stream.write_all(chunk).await.unwrap();
    }
}

async fn mixed_pair(new_source: bool, duplicate_syn: bool) {
    let mut current = kcp_sys::endpoint::KcpEndpoint::new();
    let mut legacy = KcpEndpoint::new();
    current.run().await;
    legacy.run().await;
    // Let the old endpoint's immediate first heartbeat tick run with no state.
    // Its known PING-before-SYN startup race is unrelated to mode negotiation.
    tokio::time::sleep(Duration::from_millis(1)).await;

    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut tasks = JoinSet::new();
    let mut current_output = current.output_receiver().unwrap();
    let legacy_input = legacy.input_sender();
    let packets = captured.clone();
    tasks.spawn(async move {
        while let Some(packet) = current_output.recv().await {
            let bytes = packet.inner();
            let copies = if duplicate_syn && bytes[FLAGS_OFFSET] == SYN {
                2
            } else {
                1
            };
            for _ in 0..copies {
                packets.lock().push((FromEndpoint::Current, bytes.clone()));
                if legacy_input.send(bytes.clone().into()).await.is_err() {
                    return;
                }
            }
        }
    });
    let mut legacy_output = legacy.output_receiver().unwrap();
    let current_input = current.input_sender();
    let packets = captured.clone();
    tasks.spawn(async move {
        while let Some(packet) = legacy_output.recv().await {
            let bytes = packet.inner();
            let copies = if duplicate_syn && bytes[FLAGS_OFFSET] == SYN {
                2
            } else {
                1
            };
            for _ in 0..copies {
                packets.lock().push((FromEndpoint::Legacy, bytes.clone()));
                if current_input.send(bytes.clone().into()).await.is_err() {
                    return;
                }
            }
        }
    });

    let (current_id, legacy_id) = timeout(Duration::from_secs(5), async {
        if new_source {
            tokio::try_join!(
                async {
                    current
                        .connect(
                            Duration::from_secs(1),
                            1,
                            2,
                            Bytes::from_static(b"metadata"),
                        )
                        .await
                        .map_err(anyhow::Error::from)
                },
                async { legacy.accept().await.map_err(anyhow::Error::from) },
            )
        } else {
            tokio::try_join!(
                async { current.accept().await.map_err(anyhow::Error::from) },
                async {
                    legacy
                        .connect(
                            Duration::from_secs(1),
                            1,
                            2,
                            Bytes::from_static(b"metadata"),
                        )
                        .await
                        .map_err(anyhow::Error::from)
                },
            )
        }
    })
    .await
    .expect("mixed-version handshake stalled")
    .expect("mixed-version handshake failed");
    assert_eq!(current.conn_data(&current_id).unwrap(), b"metadata"[..]);
    assert_eq!(legacy.conn_data(&legacy_id).unwrap(), b"metadata"[..]);
    let mut current_stream = kcp_sys::stream::KcpStream::new(&current, current_id).unwrap();
    let mut legacy_stream = kcp_sys_legacy::stream::KcpStream::new(&legacy, legacy_id).unwrap();

    timeout(Duration::from_secs(5), async {
        let request = vec![0x35; 256 * 1024];
        let mut received = vec![0; request.len()];
        let ((), read) = tokio::join!(
            write_chunks(&mut current_stream, &request),
            legacy_stream.read_exact(&mut received),
        );
        read.unwrap();
        assert_eq!(received, request);

        current_stream.shutdown().await.unwrap();
        // d7427c2 reports BrokenPipe for orderly receive closure.
        // Preserve that known limitation instead of asserting a new EOF API.
        let error = legacy_stream.read_u8().await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);

        let response = vec![0xa7; 256 * 1024];
        let mut received = vec![0; response.len()];
        let ((), read) = tokio::join!(
            write_chunks(&mut legacy_stream, &response),
            current_stream.read_exact(&mut received),
        );
        read.unwrap();
        assert_eq!(received, response);
        legacy_stream.shutdown().await.unwrap();
        assert_eq!(current_stream.read(&mut [0]).await.unwrap(), 0);
    })
    .await
    .expect("mixed-version data or half-close stalled");

    if duplicate_syn {
        if new_source {
            assert!(timeout(Duration::from_millis(50), legacy.accept())
                .await
                .is_err());
        } else {
            assert!(timeout(Duration::from_millis(50), current.accept())
                .await
                .is_err());
        }
    }

    let captured = captured.lock();
    assert!(captured
        .iter()
        .any(|(_, packet)| packet[FLAGS_OFFSET] == FIN));
    assert!(!captured
        .iter()
        .any(|(_, packet)| packet[FLAGS_OFFSET] & FIN_ACK != 0));
    assert!(!captured
        .iter()
        .any(|(_, packet)| packet[FLAGS_OFFSET] & RST != 0));
    for (from, packet) in captured.iter() {
        let offered_mode =
            new_source && *from == FromEndpoint::Current && packet[FLAGS_OFFSET] == SYN;
        assert_eq!(
            packet[MODE_OFFSET],
            u8::from(offered_mode),
            "unexpected mode from {from:?}, flags={:#x}",
            packet[FLAGS_OFFSET]
        );
    }
}

#[tokio::test]
async fn new_source_negotiates_legacy_with_real_old_destination() {
    mixed_pair(true, false).await;
}

#[tokio::test]
async fn real_old_source_connects_to_new_destination_without_mode_offer() {
    mixed_pair(false, false).await;
}

#[tokio::test]
async fn duplicate_syn_in_mixed_connections_does_not_reset_or_accept_twice() {
    mixed_pair(true, true).await;
    mixed_pair(false, true).await;
}
