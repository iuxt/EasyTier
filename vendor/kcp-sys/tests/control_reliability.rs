use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use bytes::Bytes;
use kcp_sys::{endpoint::KcpEndpoint, packet_def::KcpPacket, stream::KcpStream};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinSet,
    time::timeout,
};

// Keep faults between real endpoints: the filter changes only wire delivery,
// never the connection state or application timeout.
struct Network {
    client: KcpEndpoint,
    server: KcpEndpoint,
    _forwarders: JoinSet<()>,
}

impl Network {
    async fn new(
        client_filter: impl FnMut(KcpPacket) -> Vec<KcpPacket> + Send + 'static,
        server_filter: impl FnMut(KcpPacket) -> Vec<KcpPacket> + Send + 'static,
    ) -> Self {
        let mut client = KcpEndpoint::new();
        let mut server = KcpEndpoint::new();
        client.run().await;
        server.run().await;
        let mut forwarders = JoinSet::new();
        for (mut output, input, mut filter) in [
            (
                client.output_receiver().unwrap(),
                server.input_sender(),
                Box::new(client_filter) as Box<dyn FnMut(KcpPacket) -> Vec<KcpPacket> + Send>,
            ),
            (
                server.output_receiver().unwrap(),
                client.input_sender(),
                Box::new(server_filter),
            ),
        ] {
            forwarders.spawn(async move {
                while let Some(packet) = output.recv().await {
                    for packet in filter(packet) {
                        if input.send(packet).await.is_err() {
                            return;
                        }
                    }
                }
            });
        }
        Self {
            client,
            server,
            _forwarders: forwarders,
        }
    }

    async fn streams(&self) -> (KcpStream, KcpStream) {
        let connected = self
            .client
            .connect(
                Duration::from_secs(1),
                1,
                2,
                Bytes::from_static(b"metadata"),
            )
            .await
            .expect("client handshake failed");
        let accepted = self.server.accept().await.expect("server accept failed");
        assert_eq!(connected, accepted);
        (
            KcpStream::new(&self.client, connected).unwrap(),
            KcpStream::new(&self.server, accepted).unwrap(),
        )
    }
}

fn drop_packets(
    count: Arc<AtomicUsize>,
    limit: usize,
    matches: impl Fn(&KcpPacket) -> bool + Send + 'static,
) -> impl FnMut(KcpPacket) -> Vec<KcpPacket> + Send {
    move |packet| {
        if matches(&packet) && count.load(Ordering::Relaxed) < limit {
            count.fetch_add(1, Ordering::Relaxed);
            Vec::new()
        } else {
            vec![packet]
        }
    }
}

fn is_final_ack(packet: &KcpPacket) -> bool {
    packet.header().is_ack() && packet.header().is_data() && packet.payload().is_empty()
}

#[tokio::test]
async fn lost_syn_recovers_with_original_connection_metadata() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let network = Network::new(
        drop_packets(dropped.clone(), 2, |packet| {
            packet.header().is_syn() && !packet.header().is_ack()
        }),
        |packet| vec![packet],
    )
    .await;
    timeout(Duration::from_secs(5), async {
        let (mut client, mut server) = network.streams().await;
        assert_eq!(server.conn_data().as_ref(), b"metadata");
        server.write_all(b"greeting").await.unwrap();
        let mut greeting = [0; 8];
        client.read_exact(&mut greeting).await.unwrap();
        assert_eq!(&greeting, b"greeting");
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
    })
    .await
    .expect("two lost SYN packets did not recover");
}

#[tokio::test]
async fn lost_synack_recovers_without_duplicate_accept() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let network = Network::new(
        |packet| vec![packet],
        drop_packets(dropped.clone(), 2, |packet| {
            packet.header().is_syn() && packet.header().is_ack()
        }),
    )
    .await;
    timeout(Duration::from_secs(5), async {
        let (mut client, mut server) = network.streams().await;
        client.write_all(b"request").await.unwrap();
        let mut request = [0; 7];
        server.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"request");
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
        assert!(timeout(Duration::from_millis(50), network.server.accept())
            .await
            .is_err());
    })
    .await
    .expect("two lost SYNACK packets did not recover");
}

#[tokio::test]
async fn lost_final_ack_then_empty_fin_completes_handshake() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let network = Network::new(
        // Drop every empty handshake ACK so that FIN itself must establish
        // the server connection, rather than a later SYNACK retry doing so.
        drop_packets(dropped.clone(), usize::MAX, is_final_ack),
        |packet| vec![packet],
    )
    .await;
    timeout(Duration::from_secs(5), async {
        let id = network
            .client
            .connect(Duration::from_secs(1), 1, 2, Bytes::new())
            .await
            .unwrap();
        let mut client = KcpStream::new(&network.client, id).unwrap();
        client.shutdown().await.unwrap();
        assert_eq!(network.server.accept().await.unwrap(), id);
        let mut server = KcpStream::new(&network.server, id).unwrap();
        let mut request = Vec::new();
        server.read_to_end(&mut request).await.unwrap();
        assert!(request.is_empty());
        server.write_all(b"response after EOF").await.unwrap();
        server.shutdown().await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(&response, b"response after EOF");
        assert!(dropped.load(Ordering::Relaxed) >= 1);
    })
    .await
    .expect("FIN did not complete the handshake after lost ACK");
}

#[tokio::test]
async fn lost_fin_recovers_eof_without_closing_reverse_direction() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let network = Network::new(
        drop_packets(dropped.clone(), 1, |packet| packet.header().is_fin()),
        |packet| vec![packet],
    )
    .await;
    timeout(Duration::from_secs(5), async {
        let (mut client, mut server) = network.streams().await;
        client.write_all(b"request before FIN").await.unwrap();
        client.shutdown().await.unwrap();
        let mut request = Vec::new();
        server.read_to_end(&mut request).await.unwrap();
        assert_eq!(&request, b"request before FIN");
        server.write_all(b"response after EOF").await.unwrap();
        server.shutdown().await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(&response, b"response after EOF");
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    })
    .await
    .expect("lost FIN did not recover EOF and reverse response");
}

#[tokio::test]
async fn lost_fin_ack_retries_without_closing_reverse_direction() {
    let acknowledgements = Arc::new(AtomicUsize::new(0));
    let count = acknowledgements.clone();
    let network = Network::new(
        |packet| vec![packet],
        move |packet| {
            if packet.header().is_fin_ack() {
                assert_eq!(packet.header().version(), 1);
                if count.fetch_add(1, Ordering::Relaxed) == 0 {
                    return Vec::new();
                }
            }
            vec![packet]
        },
    )
    .await;
    timeout(Duration::from_secs(5), async {
        let (mut client, mut server) = network.streams().await;
        client.shutdown().await.unwrap();
        assert_eq!(server.read(&mut [0; 1]).await.unwrap(), 0);
        while acknowledgements.load(Ordering::Relaxed) < 2 {
            tokio::task::yield_now().await;
        }
        server.write_all(b"still writable").await.unwrap();
        let mut response = [0; 14];
        client.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"still writable");
        server.shutdown().await.unwrap();
        assert_eq!(client.read(&mut [0; 1]).await.unwrap(), 0);
    })
    .await
    .expect("lost FIN_ACK did not recover while keeping reverse direction open");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simultaneous_close_with_lost_fin_acks_preserves_buffered_data() {
    let client_acks = Arc::new(AtomicUsize::new(0));
    let server_acks = Arc::new(AtomicUsize::new(0));
    let drop_first_ack = |count: Arc<AtomicUsize>| {
        move |packet: KcpPacket| {
            if packet.header().is_fin_ack() && count.fetch_add(1, Ordering::Relaxed) == 0 {
                return Vec::new();
            }
            vec![packet]
        }
    };
    let network = Network::new(
        drop_first_ack(client_acks.clone()),
        drop_first_ack(server_acks.clone()),
    )
    .await;
    timeout(Duration::from_secs(5), async {
        let (mut client, mut server) = network.streams().await;
        let request: Vec<_> = (0..200 * 1024).map(|i| (i % 251) as u8).collect();
        let response: Vec<_> = (0..200 * 1024).map(|i| (i % 241) as u8).collect();
        tokio::try_join!(
            async {
                for chunk in request.chunks(16 * 1024) {
                    client.write_all(chunk).await?;
                }
                client.shutdown().await
            },
            async {
                for chunk in response.chunks(16 * 1024) {
                    server.write_all(chunk).await?;
                }
                server.shutdown().await
            },
        )
        .unwrap();
        // Let both endpoints acknowledge FIN before the application drains
        // its receive buffers. A FIN_ACK must not free unread data.
        while client_acks.load(Ordering::Relaxed) < 2 || server_acks.load(Ordering::Relaxed) < 2 {
            tokio::task::yield_now().await;
        }
        let mut received_request = Vec::new();
        let mut received_response = Vec::new();
        tokio::try_join!(
            server.read_to_end(&mut received_request),
            client.read_to_end(&mut received_response),
        )
        .unwrap();
        assert_eq!(received_request, request);
        assert_eq!(received_response, response);
    })
    .await
    .expect("simultaneous FIN retries lost buffered data or EOF");
}

#[tokio::test]
async fn duplicate_handshakes_after_half_close_do_not_reopen_or_accept_twice() {
    let resets = Arc::new(AtomicUsize::new(0));
    let count = resets.clone();
    let client_count = resets.clone();
    let mut syn = None;
    let mut synack = None;
    let network = Network::new(
        move |packet| {
            if packet.header().is_rst() {
                client_count.fetch_add(1, Ordering::Relaxed);
            }
            if packet.header().is_syn() {
                syn = Some(packet.clone());
            }
            if packet.header().is_fin() {
                // Deliver the original SYN after FIN, so its duplicate is
                // observed in PeerClosed instead of the handshake state.
                return vec![packet, syn.clone().unwrap()];
            }
            if is_final_ack(&packet) {
                return vec![packet.clone(), packet];
            }
            vec![packet]
        },
        move |packet| {
            if packet.header().is_rst() {
                count.fetch_add(1, Ordering::Relaxed);
            }
            if packet.header().is_syn() && packet.header().is_ack() {
                synack = Some(packet.clone());
                return vec![packet.clone(), packet];
            }
            if packet.header().is_fin_ack() {
                return vec![packet, synack.clone().unwrap()];
            }
            vec![packet]
        },
    )
    .await;
    timeout(Duration::from_secs(5), async {
        let (mut client, mut server) = network.streams().await;
        client.shutdown().await.unwrap();
        assert_eq!(server.read(&mut [0; 1]).await.unwrap(), 0);
        server.write_all(b"reply").await.unwrap();
        server.shutdown().await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(&response, b"reply");
        assert_eq!(server.read(&mut [0; 1]).await.unwrap(), 0);
        assert!(
            timeout(Duration::from_millis(250), network.server.accept())
                .await
                .is_err(),
            "duplicate handshake accepted twice"
        );
        assert_eq!(resets.load(Ordering::Relaxed), 0);
    })
    .await
    .expect("duplicate handshake changed half-close semantics");
}

#[tokio::test]
async fn lost_final_ack_then_reordered_data_is_delivered_once() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let count = dropped.clone();
    let reordered = Arc::new(AtomicUsize::new(0));
    let reorder_count = reordered.clone();
    let mut held = None;
    let network = Network::new(
        move |packet| {
            if is_final_ack(&packet) {
                count.fetch_add(1, Ordering::Relaxed);
                return Vec::new();
            }
            if packet.header().is_data() && reorder_count.load(Ordering::Relaxed) == 0 {
                if let Some(first) = held.take() {
                    reorder_count.fetch_add(1, Ordering::Relaxed);
                    return vec![packet.clone(), first, packet];
                }
                held = Some(packet);
                return Vec::new();
            }
            vec![packet]
        },
        |packet| vec![packet],
    )
    .await;
    timeout(Duration::from_secs(5), async {
        let id = network
            .client
            .connect(Duration::from_secs(1), 1, 2, Bytes::new())
            .await
            .unwrap();
        let mut client = KcpStream::new(&network.client, id).unwrap();
        let request: Vec<_> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
        for chunk in request.chunks(16 * 1024) {
            client.write_all(chunk).await.unwrap();
        }
        client.shutdown().await.unwrap();
        assert_eq!(network.server.accept().await.unwrap(), id);
        let mut server = KcpStream::new(&network.server, id).unwrap();
        let mut received = Vec::new();
        server.read_to_end(&mut received).await.unwrap();
        assert_eq!(received, request);
        assert!(dropped.load(Ordering::Relaxed) >= 1);
        assert_eq!(reordered.load(Ordering::Relaxed), 1);
        assert!(timeout(Duration::from_millis(50), network.server.accept())
            .await
            .is_err());
    })
    .await
    .expect("data completing the handshake was lost, duplicated, or reordered in the stream");
}

#[tokio::test]
async fn acknowledged_fin_does_not_expire_a_live_half_closed_stream() {
    let network = Network::new(|packet| vec![packet], |packet| vec![packet]).await;
    let (mut client, mut server) = timeout(Duration::from_secs(5), network.streams())
        .await
        .unwrap();
    timeout(Duration::from_secs(5), async {
        client.shutdown().await.unwrap();
        assert_eq!(server.read(&mut [0; 1]).await.unwrap(), 0);
        // This response follows FIN_ACK on the same endpoint output queue.
        // Reading it ensures the client has processed the acknowledgement.
        server.write_all(b"before").await.unwrap();
        client.read_exact(&mut [0; 6]).await.unwrap();
    })
    .await
    .unwrap();

    tokio::time::pause();
    for _ in 0..61 {
        tokio::time::advance(Duration::from_secs(1)).await;
        // Continue delivering keepalives while the control clock advances.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }
    tokio::time::resume();

    timeout(Duration::from_secs(5), async {
        server.write_all(b"after FIN deadline").await.unwrap();
        server.shutdown().await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(&response, b"after FIN deadline");
    })
    .await
    .expect("acknowledged FIN incorrectly expired the reverse stream");
}

#[tokio::test]
async fn pong_and_invalid_synack_cannot_confirm_the_handshake() {
    for invalid_kind in 0..3 {
        let (valid_sender, valid_receiver) = tokio::sync::oneshot::channel();
        let mut valid_sender = Some(valid_sender);
        let network = Network::new(
            |packet| vec![packet],
            move |packet| {
                if packet.header().is_syn() && packet.header().is_ack() {
                    let mut invalid = packet.clone();
                    if let Some(sender) = valid_sender.take() {
                        sender.send(packet).unwrap();
                    }
                    match invalid_kind {
                        0 => {
                            invalid
                                .mut_header()
                                .set_syn(false)
                                .set_ack(false)
                                .set_ping(true)
                                .set_pong(true);
                        }
                        1 => {
                            invalid.mut_header().set_fin(true);
                        }
                        2 => {
                            let mut with_payload = KcpPacket::new_with_payload(b"invalid");
                            *with_payload.mut_header() = invalid.header().clone();
                            invalid = with_payload;
                        }
                        _ => unreachable!(),
                    }
                    return vec![invalid];
                }
                vec![packet]
            },
        )
        .await;
        timeout(Duration::from_secs(5), async {
            let mut connecting = Box::pin(network.client.connect(
                Duration::from_secs(1),
                1,
                2,
                Bytes::new(),
            ));
            let valid_synack = tokio::select! {
                result = &mut connecting => panic!("invalid reply completed connect: {result:?}"),
                packet = valid_receiver => packet.unwrap(),
            };
            assert!(
                timeout(Duration::from_millis(20), &mut connecting)
                    .await
                    .is_err(),
                "invalid reply confirmed protocol capabilities"
            );
            network
                .client
                .input_sender()
                .send(valid_synack)
                .await
                .unwrap();
            let id = connecting.await.unwrap();
            assert_eq!(network.server.accept().await.unwrap(), id);
        })
        .await
        .expect("a later valid SYNACK could not complete the handshake");
    }
}

#[tokio::test]
async fn unknown_synack_version_reports_protocol_error() {
    let network = Network::new(
        |packet| vec![packet],
        |mut packet| {
            if packet.header().is_syn() && packet.header().is_ack() {
                packet.mut_header().set_version(2);
            }
            vec![packet]
        },
    )
    .await;
    let error = timeout(
        Duration::from_secs(5),
        network
            .client
            .connect(Duration::from_secs(1), 1, 2, Bytes::new()),
    )
    .await
    .expect("unknown version left connect hanging")
    .expect_err("unknown version unexpectedly established a connection");
    assert!(
        format!("{error:?}").contains("InvalidProtocolVersion"),
        "{error:?}"
    );
}

#[tokio::test]
async fn lost_final_ack_recovers_accept_and_server_greeting() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let count = dropped.clone();
    let network = Network::new(
        move |packet| {
            let header = packet.header();
            if header.is_ack()
                && header.is_data()
                && packet.payload().is_empty()
                && count.load(Ordering::Relaxed) == 0
            {
                count.fetch_add(1, Ordering::Relaxed);
                return Vec::new();
            }
            vec![packet]
        },
        |packet| vec![packet],
    )
    .await;

    timeout(Duration::from_secs(5), async {
        let connected = network
            .client
            .connect(
                Duration::from_secs(1),
                1,
                2,
                Bytes::from_static(b"metadata"),
            )
            .await
            .expect("client handshake failed");
        let mut client = KcpStream::new(&network.client, connected).unwrap();
        let accepted = network.server.accept().await.expect("server accept failed");
        assert_eq!(connected, accepted);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        let mut server = KcpStream::new(&network.server, accepted).unwrap();
        assert_eq!(server.conn_data().as_ref(), b"metadata");
        server.write_all(b"greeting").await.unwrap();
        let mut greeting = [0; 8];
        client.read_exact(&mut greeting).await.unwrap();
        assert_eq!(&greeting, b"greeting");
        assert!(
            timeout(Duration::from_millis(50), network.server.accept())
                .await
                .is_err(),
            "connection accepted twice"
        );
    })
    .await
    .expect("lost final ACK prevented accept and server greeting within 5 seconds");
}
