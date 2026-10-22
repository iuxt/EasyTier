use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering},
    Arc,
};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use dashmap::DashMap;
use parking_lot::Mutex;
use tokio::{
    select,
    sync::Notify,
    task::JoinSet,
    time::{timeout, Instant},
};
use tracing::Instrument;

use crate::{
    error::Error,
    ffi_safe::{Kcp, KcpConfig},
    packet_def::KcpPacket,
    state::{KcpConnectionFSM, PacketHeaderFlagManipulator},
};

pub type Sender<T> = tokio::sync::mpsc::Sender<T>;
pub type Receiver<T> = tokio::sync::mpsc::Receiver<T>;

pub type KcpPakcetSender = Sender<KcpPacket>;
pub type KcpPacketReceiver = Receiver<KcpPacket>;

pub type KcpStreamSender = Sender<BytesMut>;
pub type KcpStreamReceiver = Receiver<BytesMut>;

const RECV_OPEN: u8 = 0;
const RECV_FIN: u8 = 1;
pub(crate) const RECV_EOF: u8 = 2;
const RECV_RESET: u8 = 3;

const CONTROL_TIMEOUT: Duration = Duration::from_secs(60);
const CONTROL_RETRY: Duration = Duration::from_millis(200);
const CLOSE_RETENTION: Duration = Duration::from_secs(65);
const REPLY_SYN_ACK: u8 = 1;
const REPLY_ACK: u8 = 2;
const REPLY_FIN_ACK: u8 = 4;

struct PendingControl {
    packet: KcpPacket,
    next_send: Instant,
    deadline: Instant,
    interval: Duration,
}

impl PendingControl {
    fn new(packet: KcpPacket, duration: Duration) -> Self {
        let now = Instant::now();
        Self {
            packet,
            next_send: now,
            deadline: now + duration,
            interval: CONTROL_RETRY,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnId {
    conv: u32,
    src_session_id: u32,
    dst_session_id: u32,
}

impl From<&KcpPacket> for ConnId {
    fn from(packet: &KcpPacket) -> Self {
        Self {
            conv: packet.header().conv(),
            src_session_id: packet.header().src_session_id(),
            dst_session_id: packet.header().dst_session_id(),
        }
    }
}

impl ConnId {
    fn fill_packet_header(&self, packet: &mut KcpPacket) {
        packet
            .mut_header()
            .set_conv(self.conv)
            .set_src_session_id(self.src_session_id)
            .set_dst_session_id(self.dst_session_id);
    }
}

struct KcpConnectionInner {
    update_notifier: Notify,
    recv_notifier: Notify,
    send_notifier: Notify,

    has_new_input: AtomicBool,
    waiting_new_send_window: AtomicBool,
    recv_done: AtomicBool,
}

struct KcpConnection {
    conn_id: ConnId,
    kcp: Arc<Mutex<Box<Kcp>>>,

    inner: Arc<KcpConnectionInner>,

    send_sender: Option<Sender<BytesMut>>,
    send_receiver: Option<Receiver<BytesMut>>,

    recv_sender: Option<Sender<BytesMut>>,
    recv_receiver: Option<Receiver<BytesMut>>,

    send_close_notifier: Arc<Notify>,
    recv_state: Arc<AtomicU8>,

    tasks: JoinSet<()>,
}

impl KcpConnection {
    pub fn new(conn_id: ConnId) -> Result<Self, Error> {
        let kcp = Kcp::new(KcpConfig::new_turbo(conn_id.conv))?;

        let (send_sender, send_receiver) = tokio::sync::mpsc::channel(128);
        let (recv_sender, recv_receiver) = tokio::sync::mpsc::channel(128);

        Ok(Self {
            conn_id,
            kcp: Arc::new(Mutex::new(kcp)),

            inner: Arc::new(KcpConnectionInner {
                update_notifier: Notify::new(),
                recv_notifier: Notify::new(),
                send_notifier: Notify::new(),

                has_new_input: AtomicBool::new(false),
                waiting_new_send_window: AtomicBool::new(false),
                recv_done: AtomicBool::new(false),
            }),

            send_sender: Some(send_sender),
            send_receiver: Some(send_receiver),

            recv_sender: Some(recv_sender),
            recv_receiver: Some(recv_receiver),

            send_close_notifier: Arc::new(Notify::new()),
            recv_state: Arc::new(AtomicU8::new(RECV_OPEN)),

            tasks: JoinSet::new(),
        })
    }

    pub fn run(&mut self, output_sender: KcpPakcetSender, version: u8) {
        let conn_id = self.conn_id;
        self.kcp
            .lock()
            .set_output_cb(Box::new(move |conv, data: BytesMut| {
                let mut kcp_packet = KcpPacket::new_with_payload(&data);
                conn_id.fill_packet_header(&mut kcp_packet);
                kcp_packet
                    .mut_header()
                    .set_data(true)
                    .set_ack(true)
                    .set_version(version);
                tracing::trace!(?conv, "sending output data: {:?}", kcp_packet);
                if let Err(e) = output_sender.try_send(kcp_packet) {
                    tracing::debug!(?e, ?conn_id, "send output data failed");
                }
                Ok(())
            }));

        // kcp updater
        let inner = self.inner.clone();
        let kcp = self.kcp.clone();
        let recv_state = self.recv_state.clone();
        self.tasks.spawn(async move {
            loop {
                let next_update_ms = kcp.lock().next_update_delay_ms();
                select! {
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(next_update_ms as u64)) => {}
                    _ = inner.update_notifier.notified() => {}
                }

                kcp.lock().update();

                if inner.has_new_input.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    inner.recv_notifier.notify_one();
                }

                if inner.waiting_new_send_window.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    inner.send_notifier.notify_one();
                }

                if recv_state.load(Ordering::Acquire) != RECV_OPEN {
                    inner.recv_notifier.notify_one();
                }
            }
        });

        // handle packet send
        let kcp = self.kcp.clone();
        let inner = self.inner.clone();
        let mut send_receiver = self.send_receiver.take().unwrap();
        let send_close_notifier = self.send_close_notifier.clone();
        self.tasks.spawn(
            async move {
                while let Some(data) = send_receiver.recv().await {
                    loop {
                        let (waitsnd, sndwnd) = {
                            let kcp = kcp.lock();
                            (kcp.waitsnd(), kcp.sendwnd())
                        };
                        if waitsnd > 2 * sndwnd {
                            inner
                                .waiting_new_send_window
                                .store(true, std::sync::atomic::Ordering::SeqCst);
                            inner.send_notifier.notified().await;
                        } else {
                            break;
                        }
                    }
                    kcp.lock().send(data.freeze()).unwrap();
                    kcp.lock().flush();
                    inner.update_notifier.notify_one();
                }

                tracing::debug!(
                    ?conn_id,
                    "connection packet sender close, waiting for waitsnd to be 0"
                );

                // waiting for waitsnd to be 0
                while kcp.lock().waitsnd() > 0 {
                    inner
                        .waiting_new_send_window
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    inner.send_notifier.notified().await;
                }

                send_close_notifier.notify_one();
                tracing::debug!(?conn_id, "connection packet send task done");
            }
            .instrument(tracing::trace_span!("send_task", conn = ?conn_id)),
        );

        // handle packet recv
        let kcp = self.kcp.clone();
        let inner = self.inner.clone();
        let conn_id = self.conn_id;
        let recv_sender = self.recv_sender.take().unwrap();
        let recv_state = self.recv_state.clone();
        self.tasks.spawn(
            async move {
                let mut buf = BytesMut::new();
                loop {
                    // Observe FIN before inspecting the queue: a FIN arriving
                    // after an empty peek must not make that stale peek EOF.
                    let state = recv_state.load(Ordering::Acquire);
                    let peeksize = kcp.lock().peeksize();
                    if peeksize <= 0 {
                        // FIN is sent only after KCP acknowledges all bytes. Drain
                        // those bytes before publishing EOF to the stream.
                        if state != RECV_OPEN {
                            let _ = recv_state.compare_exchange(
                                RECV_FIN,
                                RECV_EOF,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            );
                            break;
                        }
                        tracing::trace!("recv nothing, wait for next update");
                        inner.recv_notifier.notified().await;
                        continue;
                    };

                    if buf.capacity() < peeksize as usize {
                        buf.reserve(std::cmp::max(peeksize as usize, 4096));
                    }
                    kcp.lock().recv(&mut buf).unwrap();
                    tracing::trace!("recv data ({}): {:?}", buf.len(), buf);
                    assert_ne!(0, buf.len());
                    let send_ret = recv_sender.send(buf.split()).await;
                    if send_ret.is_err() {
                        break;
                    }
                }

                inner.recv_done.store(true, Ordering::Release);
                tracing::debug!(?conn_id, "connection packet recv task done");
            }
            .instrument(tracing::trace_span!("recv_task", conn = ?conn_id)),
        );
    }

    fn handle_input(&mut self, packet: &KcpPacket) -> Result<(), Error> {
        self.kcp.lock().handle_input(packet.payload())?;
        self.inner
            .has_new_input
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.update_notifier.notify_one();
        Ok(())
    }

    fn send_sender(&mut self) -> KcpStreamSender {
        self.send_sender.take().unwrap()
    }

    fn recv_receiver(&mut self) -> KcpStreamReceiver {
        self.recv_receiver.take().unwrap()
    }

    fn send_close_notifier(&self) -> Arc<Notify> {
        self.send_close_notifier.clone()
    }

    fn close_recv(&self, reset: bool) {
        if reset {
            self.recv_state.store(RECV_RESET, Ordering::Release);
        } else {
            let _ = self.recv_state.compare_exchange(
                RECV_OPEN,
                RECV_FIN,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        self.inner.recv_notifier.notify_one();
    }

    fn recv_drained(&self) -> bool {
        self.inner.recv_done.load(Ordering::Acquire)
    }
}

impl Drop for KcpConnection {
    fn drop(&mut self) {
        self.send_close_notifier.notify_one();
    }
}

impl PacketHeaderFlagManipulator for KcpPacket {
    fn has_syn(&self) -> bool {
        self.header().is_syn()
    }

    fn has_ack(&self) -> bool {
        self.header().is_ack()
    }

    fn has_fin(&self) -> bool {
        self.header().is_fin()
    }

    fn has_rst(&self) -> bool {
        self.header().is_rst()
    }

    fn has_data(&self) -> bool {
        self.header().is_data()
    }

    fn set_syn(&mut self, value: bool) {
        self.mut_header().set_syn(value);
    }

    fn set_ack(&mut self, value: bool) {
        self.mut_header().set_ack(value);
    }

    fn set_fin(&mut self, value: bool) {
        self.mut_header().set_fin(value);
    }

    fn set_rst(&mut self, value: bool) {
        self.mut_header().set_rst(value);
    }

    fn set_data(&mut self, value: bool) {
        self.mut_header().set_data(value);
    }
}

struct KcpConnectionState {
    fsm: KcpConnectionFSM,
    notify: Arc<Notify>,
    conn_data: Bytes,
    last_pong: std::time::Instant,
    pending_accept: bool,
    version: u8,
    control: Option<PendingControl>,
    replies: u8,
    reply_deadline: Option<Instant>,
    next_reply: Instant,
    fin_sent: bool,
    fin_acked: bool,
    closed_until: Option<Instant>,
    reset: bool,
}

impl std::fmt::Debug for KcpConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KcpConnectionState")
            .field("fsm", &self.fsm)
            .finish()
    }
}

impl KcpConnectionState {
    fn new(fsm: KcpConnectionFSM) -> Self {
        Self {
            fsm,
            notify: Arc::new(Notify::new()),
            conn_data: Bytes::new(),
            last_pong: std::time::Instant::now(),
            pending_accept: false,
            version: 0,
            control: None,
            replies: 0,
            reply_deadline: None,
            next_reply: Instant::now(),
            fin_sent: false,
            fin_acked: false,
            closed_until: None,
            reset: false,
        }
    }

    fn reply(&mut self, reply: u8) {
        if self.replies == 0 {
            self.reply_deadline = Some(Instant::now() + CONTROL_TIMEOUT);
            self.next_reply = Instant::now();
        }
        self.replies |= reply;
    }

    fn accepts(&self, packet: &KcpPacket) -> bool {
        if self.version == 0 {
            return !packet.header().is_fin_ack();
        }
        let hdr = packet.header();
        if !Self::valid_reliable_packet(packet) {
            return false;
        }
        if matches!(self.fsm, KcpConnectionFSM::SynSent) {
            return hdr.version() <= 1 && (Self::is_syn_ack(packet) || hdr.is_rst());
        }
        hdr.version() == 1
    }

    fn valid_reliable_packet(packet: &KcpPacket) -> bool {
        let flags = packet.header().flags();
        matches!(flags, 1 | 3 | 4 | 10 | 16 | 32 | 96 | 128)
            && (matches!(flags, 1 | 10) || packet.payload().is_empty())
    }

    fn is_syn_ack(packet: &KcpPacket) -> bool {
        packet.header().flags() == 3 && packet.payload().is_empty()
    }

    fn handle_packet(&mut self, packet: &KcpPacket) -> Result<Option<KcpPacket>, Error> {
        self.notify_pong();
        if self.version == 1 {
            return self.handle_reliable_packet(packet);
        }
        let mut out_packet = None;
        let old_state = self.fsm;
        let _ = self.fsm.handle_packet(packet, &mut out_packet);
        if old_state != self.fsm {
            self.notify.notify_one();
            return Ok(out_packet);
        }
        Ok(None)
    }

    fn handle_reliable_packet(&mut self, packet: &KcpPacket) -> Result<Option<KcpPacket>, Error> {
        let hdr = packet.header();
        if self.closed_until.is_some() {
            if hdr.is_fin() && packet.payload().is_empty() {
                self.reply(REPLY_FIN_ACK);
            }
            return Ok(None);
        }
        if hdr.is_fin_ack() {
            if packet.payload().is_empty() && self.fin_sent {
                self.fin_acked = true;
                self.control = None;
            }
            return Ok(None);
        }
        if Self::is_syn_ack(packet) {
            if matches!(self.fsm, KcpConnectionFSM::SynSent) {
                self.version = hdr.version();
                self.control = None;
                let mut out = None;
                self.fsm.handle_packet(packet, &mut out)?;
                self.notify.notify_one();
                if self.version == 0 {
                    return Ok(out);
                }
            }
            self.reply(REPLY_ACK);
            return Ok(None);
        }
        if hdr.is_syn()
            && !hdr.is_ack()
            && !hdr.is_data()
            && !hdr.is_fin()
            && !hdr.is_rst()
            && packet.payload() == self.conn_data.as_ref()
        {
            self.reply(REPLY_SYN_ACK);
            return Ok(None);
        }
        if hdr.is_fin()
            && !hdr.is_syn()
            && !hdr.is_ack()
            && !hdr.is_data()
            && !hdr.is_rst()
            && packet.payload().is_empty()
        {
            if matches!(self.fsm, KcpConnectionFSM::SynReceived) {
                // Empty streams can close before their final handshake ACK arrives.
                self.fsm = KcpConnectionFSM::PeerClosed;
                self.control = None;
            } else if matches!(self.fsm, KcpConnectionFSM::Established) {
                self.fsm = KcpConnectionFSM::PeerClosed;
            } else if matches!(self.fsm, KcpConnectionFSM::LocalClosed) {
                self.fsm = KcpConnectionFSM::Closed;
            }
            self.reply(REPLY_FIN_ACK);
            return Ok(None);
        }
        let old = self.fsm;
        let mut out = None;
        let ret = self.fsm.handle_packet(packet, &mut out);
        if hdr.is_rst() {
            self.reset = true;
        }
        if old != self.fsm {
            if matches!(old, KcpConnectionFSM::SynReceived) || hdr.is_rst() {
                self.control = None;
            }
            self.notify.notify_one();
        }
        ret?;
        Ok(out)
    }

    fn notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    fn is_established(&self) -> bool {
        matches!(self.fsm, KcpConnectionFSM::Established)
    }

    fn is_peer_closed(&self) -> bool {
        matches!(
            self.fsm,
            KcpConnectionFSM::PeerClosed | KcpConnectionFSM::Closed
        )
    }

    fn is_closed(&self) -> bool {
        matches!(self.fsm, KcpConnectionFSM::Closed)
    }

    fn set_data(&mut self, data: Bytes) {
        self.conn_data = data;
    }

    fn notify_pong(&mut self) {
        self.last_pong = std::time::Instant::now();
    }

    fn is_pong_timeout(&self) -> bool {
        self.last_pong.elapsed() > std::time::Duration::from_secs(60)
    }

    fn poll_control(&mut self, conn_id: ConnId, output: &KcpPakcetSender, now: Instant) -> Instant {
        let mut next = now + Duration::from_secs(10);
        if let Some(control) = self.control.as_mut() {
            if now >= control.deadline {
                tracing::debug!(?conn_id, packet = ?control.packet.header(), "control request timed out");
                self.reset = true;
                return now;
            }
            if now >= control.next_send {
                match output.try_send(control.packet.clone()) {
                    Ok(()) => {
                        self.fin_sent |= control.packet.header().is_fin();
                        control.next_send = now + control.interval;
                        control.interval = (control.interval * 2).min(Duration::from_secs(2));
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        control.next_send = now + Duration::from_millis(25);
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        self.reset = true;
                        return now;
                    }
                }
            }
            next = next.min(control.next_send).min(control.deadline);
        }
        if let Some(deadline) = self.reply_deadline {
            if now >= deadline {
                self.replies = 0;
                self.reply_deadline = None;
            } else {
                if now >= self.next_reply {
                    for reply in [REPLY_SYN_ACK, REPLY_ACK, REPLY_FIN_ACK] {
                        if self.replies & reply == 0 {
                            continue;
                        }
                        let mut packet = KcpPacket::new(0);
                        conn_id.fill_packet_header(&mut packet);
                        packet.mut_header().set_version(1);
                        match reply {
                            REPLY_SYN_ACK => {
                                packet.mut_header().set_syn(true).set_ack(true);
                            }
                            REPLY_ACK => {
                                packet.mut_header().set_ack(true).set_data(true);
                            }
                            REPLY_FIN_ACK => {
                                packet.mut_header().set_fin_ack();
                            }
                            _ => unreachable!(),
                        }
                        match output.try_send(packet) {
                            Ok(()) => self.replies &= !reply,
                            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => break,
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                self.reset = true;
                                return now;
                            }
                        }
                    }
                    self.next_reply = now + Duration::from_millis(25);
                }
                if self.replies == 0 {
                    self.reply_deadline = None;
                } else {
                    next = next.min(self.next_reply).min(deadline);
                }
            }
        }
        next
    }
}

struct KcpEndpointData {
    cur_conv: AtomicU32,
    conn_map: DashMap<ConnId, KcpConnection>,
    state_map: DashMap<ConnId, KcpConnectionState>,
    control_notify: Notify,
}

impl KcpEndpointData {
    fn new() -> Self {
        Self {
            cur_conv: AtomicU32::new(rand::random()),
            conn_map: DashMap::new(),
            state_map: DashMap::new(),
            control_notify: Notify::new(),
        }
    }
}

struct Connecting<'a> {
    data: &'a KcpEndpointData,
    conn_id: ConnId,
    notify: Arc<Notify>,
    complete: bool,
}

impl Drop for Connecting<'_> {
    fn drop(&mut self) {
        if self.complete {
            return;
        }
        if let dashmap::mapref::entry::Entry::Occupied(entry) =
            self.data.state_map.entry(self.conn_id)
        {
            if Arc::ptr_eq(&entry.get().notify, &self.notify) {
                if let Some((_, conn)) = self.data.conn_map.remove(&self.conn_id) {
                    conn.close_recv(true);
                }
                entry.remove();
            }
        }
        self.data.control_notify.notify_one();
    }
}

pub type KcpConfigFactory = Box<dyn Fn(u32) -> KcpConfig + Send + Sync>;

pub struct KcpEndpoint {
    id: u64,
    data: Arc<KcpEndpointData>,

    input_sender: KcpPakcetSender,
    input_receiver: Option<KcpPacketReceiver>,

    output_sender: KcpPakcetSender,
    output_receiver: Option<KcpPacketReceiver>,

    new_conn_sender: tokio::sync::mpsc::Sender<ConnId>,
    new_conn_receiver: Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<ConnId>>>,

    kcp_config_factory: KcpConfigFactory,

    tasks: JoinSet<()>,
}

impl std::fmt::Debug for KcpEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KcpEndpoint").field("id", &self.id).finish()
    }
}

impl Default for KcpEndpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl KcpEndpoint {
    pub fn new() -> Self {
        let (input_sender, input_receiver) = tokio::sync::mpsc::channel(1024);
        let (output_sender, output_receiver) = tokio::sync::mpsc::channel(1024);
        let (new_conn_sender, new_conn_receiver) = tokio::sync::mpsc::channel(4);

        Self {
            id: rand::random(),
            data: Arc::new(KcpEndpointData::new()),

            input_sender,
            input_receiver: Some(input_receiver),

            output_sender,
            output_receiver: Some(output_receiver),

            new_conn_sender,
            new_conn_receiver: Arc::new(tokio::sync::Mutex::new(new_conn_receiver)),

            kcp_config_factory: Box::new(KcpConfig::new_turbo),

            tasks: JoinSet::new(),
        }
    }

    pub fn set_kcp_config_factory(&mut self, factory: KcpConfigFactory) {
        self.kcp_config_factory = factory;
    }

    async fn try_handle_pingpong(
        data: &KcpEndpointData,
        packet: &KcpPacket,
        output_sender: &KcpPakcetSender,
    ) -> bool {
        let hdr = packet.header();

        if hdr.is_ping() && !hdr.is_pong() {
            let conn_id = ConnId::from(packet);
            let need_send_pong = data
                .state_map
                .get_mut(&conn_id)
                .map(|x| !x.is_closed())
                .unwrap_or(false);

            let mut out_packet = packet.clone();
            if need_send_pong {
                out_packet.mut_header().set_pong(true);
            } else {
                out_packet.mut_header().set_ping(false);
                out_packet.mut_header().set_rst(true);
            };

            tracing::trace!("sending pong packet: {:?}", out_packet);
            if hdr.version() == 1 {
                // Heartbeats are periodic; a full output queue must not stall
                // control input and prevent ACK/FIN processing.
                if let Err(e) = output_sender.try_send(out_packet) {
                    tracing::debug!(?e, "send pong packet failed");
                }
            } else if let Err(e) = output_sender.send(out_packet).await {
                tracing::error!(?e, "send pong packet failed");
            }
        }

        // all incoming packet should update pong time
        let conv = ConnId::from(packet);
        if let Some(mut state) = data.state_map.get_mut(&conv) {
            state.notify_pong();
        }

        packet.header().is_ping()
    }

    pub async fn run(&mut self) {
        let mut input_receiver = self.input_receiver.take().unwrap();
        let data = self.data.clone();
        let output_sender = self.output_sender.clone();
        let new_conn_sender = self.new_conn_sender.clone();

        self.tasks.spawn(
            async move {
                while let Some(packet) = input_receiver.recv().await {
                    tracing::trace!("recv packet: {:?}", packet);
                    let conv = ConnId::from(&packet);
                    if let Some(mut state) = data.state_map.get_mut(&conv) {
                        if matches!(state.fsm, KcpConnectionFSM::SynSent)
                            && KcpConnectionState::is_syn_ack(&packet)
                            && packet.header().version() > 1
                        {
                            state.version = packet.header().version();
                            state.fsm = KcpConnectionFSM::Closed;
                            state.control = None;
                            state.notify.notify_one();
                            continue;
                        }
                        if !state.accepts(&packet)
                            || (state.version == 1
                                && state.is_closed()
                                && packet.header().is_ping())
                        {
                            continue;
                        }
                    }
                    if Self::try_handle_pingpong(&data, &packet, &output_sender).await {
                        continue;
                    }

                    let conv = ConnId::from(&packet);
                    if packet.header().is_data() && !packet.payload().is_empty() {
                        if let Some(mut conn) = data.conn_map.get_mut(&conv) {
                            if let Err(e) = conn.handle_input(&packet) {
                                tracing::error!(?e, ?conv, "handle input on connection failed");
                            } else {
                                tracing::trace!(?conv, "handle input on connection done");
                            }
                        } else {
                            tracing::debug!(
                                ?conv,
                                ?packet,
                                "no conn for conv when handling data packet"
                            );
                        }
                    }

                    let mut state_ref = data.state_map.get_mut(&conv);
                    let state = state_ref.as_deref_mut();
                    let mut out_packet: Option<KcpPacket> = None;
                    if let Some(state) = state {
                        let was_syn_received = matches!(state.fsm, KcpConnectionFSM::SynReceived);
                        let ret = state.handle_packet(&packet);
                        tracing::trace!(?conv, ?state, "handle packet for conn, ret: {:?}", ret);
                        if let Ok(pkt) = ret {
                            out_packet = pkt;
                        }

                        if was_syn_received
                            && (state.is_established()
                                || matches!(state.fsm, KcpConnectionFSM::PeerClosed))
                        {
                            // The channel is only a notification; retain ownership of
                            // every pending accept even when its capacity is exhausted.
                            state.pending_accept = true;
                            let _ = new_conn_sender.try_send(conv);
                        }

                        if state.is_peer_closed() {
                            tracing::debug!(?conv, "peer half closed, close recv");
                            if let Some(conn) = data.conn_map.get_mut(&conv) {
                                conn.close_recv(packet.header().is_rst())
                            }
                        }

                        if state.is_closed() && packet.header().is_rst() {
                            data.conn_map.remove(&conv);
                        }
                        if state.version == 1
                            && (state.replies != 0
                                || state.reset
                                || was_syn_received
                                || packet.header().is_fin_ack())
                        {
                            data.control_notify.notify_one();
                        }
                    } else {
                        if packet.header().version() == 1
                            && !KcpConnectionState::valid_reliable_packet(&packet)
                        {
                            continue;
                        }
                        if packet.header().is_rst() {
                            tracing::debug!(?conv, "reset packet for conn, but no state");
                            continue;
                        }
                        let mut tmp_fsm = KcpConnectionFSM::listen();
                        let res = tmp_fsm.handle_packet(&packet, &mut out_packet);
                        tracing::trace!(
                            ?conv,
                            ?out_packet,
                            "handle first packet for conn, ret: {:?}",
                            res
                        );
                        if res.is_ok() {
                            let mut conn_state = KcpConnectionState::new(tmp_fsm);
                            conn_state.set_data(packet.payload().to_vec().into());
                            if packet.header().version() == 1 && packet.header().flags() == 1 {
                                conn_state.version = 1;
                                let mut response = out_packet.take().unwrap();
                                conv.fill_packet_header(&mut response);
                                response.mut_header().set_version(1);
                                conn_state.control =
                                    Some(PendingControl::new(response, CONTROL_TIMEOUT));
                                data.control_notify.notify_one();
                            }
                            data.state_map.insert(conv, conn_state);
                        }
                    }

                    drop(state_ref);
                    if let Some(mut out_packet) = out_packet {
                        conv.fill_packet_header(&mut out_packet);
                        if packet.header().version() == 1 && packet.header().flags() != 1 {
                            out_packet.mut_header().set_version(1);
                        }
                        tracing::trace!(?conv, ?out_packet, "sending output packet");
                        if out_packet.header().version() == 1 {
                            // Reliable requests and replies are retained in state.
                            // A stateless reset has no retry obligation and must
                            // not block input when the output queue is saturated.
                            if let Err(e) = output_sender.try_send(out_packet) {
                                tracing::debug!(?e, "send stateless response failed");
                            }
                        } else if let Err(e) = output_sender.send(out_packet).await {
                            tracing::error!(?e, "send output packet failed");
                        }
                    }
                }
            }
            .instrument(tracing::trace_span!("recv_task", id = self.id)),
        );

        // Connection cleanup and control retransmission share one deadline loop.
        let data = self.data.clone();
        let output_sender = self.output_sender.clone();
        self.tasks.spawn(async move {
            let mut next_cleanup = Instant::now();
            loop {
                let now = Instant::now();
                let cleanup = now >= next_cleanup;
                if cleanup {
                    next_cleanup = now + Duration::from_secs(10);
                }
                let mut next = next_cleanup;
                data.state_map.retain(|conn_id, state| {
                    let closed = state.is_closed()
                        && data
                            .conn_map
                            .get(conn_id)
                            .map(|conn| conn.recv_drained())
                            .unwrap_or(true);
                    let mut remove = false;
                    if state.version == 1 {
                        if closed && state.fin_acked && state.closed_until.is_none() {
                            data.conn_map.remove(conn_id);
                            state.closed_until = Some(now + CLOSE_RETENTION);
                        }
                        if let Some(until) = state.closed_until {
                            remove = now >= until;
                            next = next.min(until);
                        } else if cleanup && state.is_pong_timeout() {
                            state.reset = true;
                        }
                        if !remove && !state.reset {
                            next = next.min(state.poll_control(*conn_id, &output_sender, now));
                        }
                        remove |= state.reset;
                    } else if cleanup {
                        remove = closed || state.is_pong_timeout();
                    }
                    if remove {
                        state.notify.notify_one();
                        // Keep state -> connection lock order and prevent ID reuse
                        // until its old data object has also been removed.
                        if let Some((_, conn)) = data.conn_map.remove(conn_id) {
                            if state.reset || !closed {
                                conn.close_recv(true);
                            }
                        }
                    }
                    !remove
                });
                if cleanup {
                    data.state_map.shrink_to_fit();
                    data.conn_map.shrink_to_fit();
                }
                select! {
                    _ = tokio::time::sleep_until(next) => {}
                    _ = data.control_notify.notified() => {}
                }
            }
        });

        // conn ping task
        let data = self.data.clone();
        let output_sender = self.output_sender.clone();
        self.tasks.spawn(async move {
            loop {
                let packets = data
                    .state_map
                    .iter()
                    .filter_map(|item| {
                        let (conn_id, state) = item.pair();
                        // Before the handshake completes, a ping can overtake
                        // SYN and make the peer reset an unknown connection.
                        if !matches!(
                            state.fsm,
                            KcpConnectionFSM::Established
                                | KcpConnectionFSM::LocalClosed
                                | KcpConnectionFSM::PeerClosed
                        ) {
                            return None;
                        }
                        let mut out_packet = KcpPacket::new(0);
                        conn_id.fill_packet_header(&mut out_packet);
                        out_packet
                            .mut_header()
                            .set_ping(true)
                            .set_version(state.version);
                        Some(out_packet)
                    })
                    .collect::<Vec<_>>();

                for packet in packets {
                    let ret = output_sender.send(packet).await;
                    if let Err(e) = ret {
                        tracing::error!(?e, "send ping packet failed");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }

                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        });
    }

    fn add_conn(&self, conn_id: ConnId, peer_closed: bool, version: u8) -> Result<(), Error> {
        let mut conn = KcpConnection::new(conn_id)?;
        // A peer may send FIN before the application claims the connection.
        if peer_closed {
            conn.close_recv(false);
        }
        conn.run(self.output_sender.clone(), version);

        let data = self.data.clone();
        let close_notifier = conn.send_close_notifier();

        data.conn_map.insert(conn_id, conn);

        let output_sender = self.output_sender.clone();
        let data = Arc::downgrade(&data);
        tokio::spawn(async move {
            close_notifier.notified().await;
            let Some(data) = data.upgrade() else {
                return;
            };
            let mut out_packet = KcpPacket::new(0);
            let Some(mut state) = data.state_map.get_mut(&conn_id) else {
                return;
            };

            if state.version == 1 && (state.reset || state.closed_until.is_some()) {
                return;
            }

            let close_ret = state.fsm.close(&mut out_packet);
            let cur_state = state.fsm;
            if state.version == 1 && !state.reset && state.closed_until.is_none() {
                conn_id.fill_packet_header(&mut out_packet);
                out_packet.mut_header().set_version(1);
                state.control = Some(PendingControl::new(out_packet, CONTROL_TIMEOUT));
                drop(state);
                data.control_notify.notify_one();
                tracing::debug!(?conn_id, ?cur_state, "waiting for FIN acknowledgement");
                return;
            }
            drop(state);
            match close_ret {
                Ok(_) => {
                    conn_id.fill_packet_header(&mut out_packet);
                    output_sender.send(out_packet).await.unwrap();
                }
                Err(e) => {
                    tracing::error!(?e, ?conn_id, "close connection failed");
                }
            }

            // A peer FIN may arrive while acknowledged bytes are still waiting
            // for the application. The periodic task removes the connection
            // once its receive task has drained them.

            tracing::debug!(?conn_id, ?cur_state, "connection close watcher done");
        });

        Ok(())
    }

    pub fn output_receiver(&mut self) -> Option<KcpPacketReceiver> {
        self.output_receiver.take()
    }

    pub fn input_sender(&self) -> KcpPakcetSender {
        self.input_sender.clone()
    }

    pub fn input_sender_ref(&self) -> &KcpPakcetSender {
        &self.input_sender
    }

    pub fn conn_sender_receiver(
        &self,
        conn_id: ConnId,
    ) -> Option<(KcpStreamSender, KcpStreamReceiver)> {
        let mut conn = self.data.conn_map.get_mut(&conn_id)?;
        Some((conn.send_sender(), conn.recv_receiver()))
    }

    pub(crate) fn conn_recv_state(&self, conn_id: ConnId) -> Option<Arc<AtomicU8>> {
        Some(self.data.conn_map.get(&conn_id)?.recv_state.clone())
    }

    pub fn conn_data(&self, conn_id: &ConnId) -> Option<Bytes> {
        let state = self.data.state_map.get(conn_id)?;
        Some(state.conn_data.clone())
    }

    #[tracing::instrument(ret)]
    pub async fn connect(
        &self,
        timeout_dur: std::time::Duration,
        src_session_id: u32,
        dst_session_id: u32,
        conn_data: Bytes,
    ) -> Result<ConnId, Error> {
        let mut out_packet = KcpPacket::new_with_payload(&conn_data);
        let fsm = KcpConnectionFSM::connect(&mut out_packet);
        let (conn_id, notify, deadline) = loop {
            let conv_cand = self
                .data
                .cur_conv
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let conn_id = ConnId {
                conv: conv_cand,
                src_session_id,
                dst_session_id,
            };
            if let dashmap::mapref::entry::Entry::Vacant(entry) = self.data.state_map.entry(conn_id)
            {
                let mut state = KcpConnectionState::new(fsm);
                state.set_data(conn_data.clone());
                let notify = state.notify();
                conn_id.fill_packet_header(&mut out_packet);
                out_packet.mut_header().set_version(1);
                state.version = 1;
                state.control = Some(PendingControl::new(out_packet, timeout_dur));
                let deadline = state.control.as_ref().unwrap().deadline;
                entry.insert(state);
                break (conn_id, notify, deadline);
            }
        };
        let mut connecting = Connecting {
            data: &self.data,
            conn_id,
            notify: notify.clone(),
            complete: false,
        };
        self.data.control_notify.notify_one();
        if timeout(timeout_dur, notify.notified()).await.is_err() {
            return Err(Error::ConnectTimeout);
        }

        if Instant::now() >= deadline {
            return Err(Error::ConnectTimeout);
        }

        if let Some(state) = self.data.state_map.get(&conn_id) {
            tracing::debug!(?conn_id, ?state, "connect done, checkin state");
            if state.version > 1 {
                return Err(Error::InvalidProtocolVersion(state.version));
            }
            if matches!(
                state.fsm,
                KcpConnectionFSM::Established | KcpConnectionFSM::PeerClosed
            ) {
                self.add_conn(conn_id, state.is_peer_closed(), state.version)?;
                connecting.complete = true;
                return Ok(conn_id);
            } else {
                drop(state);
            }
            // The connecting guard also cleans up if this future is cancelled.
        }

        return Err(anyhow::anyhow!("connect failed").into());
    }

    pub async fn accept(&self) -> Result<ConnId, Error> {
        let mut conn_receiver = self.new_conn_receiver.lock().await;

        loop {
            // Drain notifications first so the normal path does not scan the map.
            // A full channel can coalesce notifications, but cannot lose the
            // pending connection recorded in the state map.
            let conn_id = match conn_receiver.try_recv() {
                Ok(conn_id) => conn_id,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    return Err(Error::Shutdown);
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    let pending = self
                        .data
                        .state_map
                        .iter()
                        .find_map(|state| state.pending_accept.then_some(*state.key()));
                    if let Some(conn_id) = pending {
                        conn_id
                    } else {
                        conn_receiver.recv().await.ok_or(Error::Shutdown)?
                    }
                }
            };

            let Some(mut state) = self.data.state_map.get_mut(&conn_id) else {
                continue;
            };
            if !state.pending_accept {
                continue;
            }
            state.pending_accept = false;
            if matches!(
                state.fsm,
                KcpConnectionFSM::Established | KcpConnectionFSM::PeerClosed
            ) {
                self.add_conn(conn_id, state.is_peer_closed(), state.version)?;
                return Ok(conn_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context;
    use tracing::level_filters::LevelFilter;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer as _};

    use super::*;

    fn _enable_log() {
        let console_layer = tracing_subscriber::fmt::layer()
            .pretty()
            .with_writer(std::io::stderr)
            .with_filter(LevelFilter::TRACE);

        tracing_subscriber::Registry::default()
            .with(console_layer)
            .init();
    }

    async fn prepare_test() -> (KcpEndpoint, KcpEndpoint, JoinSet<()>) {
        let mut client_endpoint = KcpEndpoint::new();
        let mut server_endpoint = KcpEndpoint::new();
        let mut t = JoinSet::new();

        client_endpoint.run().await;
        server_endpoint.run().await;

        let client_input_sender = client_endpoint.input_sender();
        let mut server_output_receiver = server_endpoint.output_receiver().unwrap();
        t.spawn(async move {
            while let Some(packet) = server_output_receiver.recv().await {
                let _ = client_input_sender.send(packet).await;
            }
        });

        let server_input_sender = server_endpoint.input_sender();
        let mut client_output_receiver = client_endpoint.output_receiver().unwrap();
        t.spawn(async move {
            while let Some(packet) = client_output_receiver.recv().await {
                let _ = server_input_sender.send(packet).await;
            }
        });

        (client_endpoint, server_endpoint, t)
    }

    #[tokio::test]
    async fn test_cancelled_connect_releases_state() {
        let mut endpoint = KcpEndpoint::new();
        endpoint.run().await;
        assert!(timeout(
            Duration::from_millis(5),
            endpoint.connect(CONTROL_TIMEOUT, 1, 2, Bytes::new(),)
        )
        .await
        .is_err());
        assert!(endpoint.data.state_map.is_empty());
        assert!(endpoint.data.conn_map.is_empty());
    }

    #[tokio::test]
    async fn test_unknown_close_does_not_block_input_when_output_is_full() {
        let mut endpoint = KcpEndpoint::new();
        endpoint.run().await;
        while endpoint.output_sender.try_send(KcpPacket::new(0)).is_ok() {}
        let mut packet = KcpPacket::new(0);
        packet
            .mut_header()
            .set_conv(1)
            .set_src_session_id(2)
            .set_dst_session_id(3)
            .set_version(1)
            .set_fin(true);
        endpoint.input_sender().send(packet.clone()).await.unwrap();
        packet.mut_header().set_conv(2).set_fin(false).set_syn(true);
        let id = ConnId::from(&packet);
        endpoint.input_sender().send(packet).await.unwrap();
        timeout(Duration::from_millis(100), async {
            while !endpoint.data.state_map.contains_key(&id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("an unknown FIN must not stall the next handshake");
    }

    #[tokio::test(start_paused = true)]
    async fn test_half_open_deadline_survives_duplicates_and_full_output() {
        for full_output in [false, true] {
            let mut endpoint = KcpEndpoint::new();
            let mut output = endpoint.output_receiver().unwrap();
            endpoint.run().await;
            if full_output {
                while endpoint.output_sender.try_send(KcpPacket::new(0)).is_ok() {}
            }
            let mut syn = KcpPacket::new(0);
            syn.mut_header()
                .set_conv(1)
                .set_src_session_id(2)
                .set_dst_session_id(3)
                .set_syn(true)
                .set_version(1);
            let id = ConnId::from(&syn);
            endpoint.input_sender().send(syn.clone()).await.unwrap();
            tokio::task::yield_now().await;
            assert!(endpoint.data.state_map.contains_key(&id));
            if !full_output {
                let response = output.recv().await.unwrap();
                assert!(KcpConnectionState::is_syn_ack(&response));
            }
            for _ in 0..5 {
                tokio::time::advance(Duration::from_secs(10)).await;
                endpoint.input_sender().send(syn.clone()).await.unwrap();
                tokio::task::yield_now().await;
                assert!(endpoint.data.state_map.contains_key(&id));
            }
            tokio::time::advance(Duration::from_secs(11)).await;
            tokio::task::yield_now().await;
            assert!(!endpoint.data.state_map.contains_key(&id));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn test_fin_timeout_is_read_error_and_releases_state() {
        use crate::stream::KcpStream;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut endpoint = KcpEndpoint::new();
        let mut output = endpoint.output_receiver().unwrap();
        endpoint.run().await;
        let mut syn = KcpPacket::new(0);
        syn.mut_header()
            .set_conv(1)
            .set_src_session_id(2)
            .set_dst_session_id(3)
            .set_syn(true)
            .set_version(1);
        endpoint.input_sender().send(syn.clone()).await.unwrap();
        assert!(output.recv().await.unwrap().header().is_syn());
        syn.mut_header().set_syn(false).set_ack(true).set_data(true);
        endpoint.input_sender().send(syn).await.unwrap();
        let id = endpoint.accept().await.unwrap();
        let mut stream = KcpStream::new(&endpoint, id).unwrap();
        stream.shutdown().await.unwrap();
        loop {
            if output.recv().await.unwrap().header().is_fin() {
                break;
            }
        }
        tokio::time::advance(CONTROL_TIMEOUT + Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            stream.read(&mut [0]).await.unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert!(!endpoint.data.state_map.contains_key(&id));
        assert!(!endpoint.data.conn_map.contains_key(&id));
    }

    #[tokio::test(start_paused = true)]
    async fn test_closed_record_handles_duplicates_and_prevents_id_reuse() {
        use crate::stream::KcpStream;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (client, server, _tasks) = prepare_test().await;
        let id = connect_test_pair(&client, &server).await;
        let mut a = KcpStream::new(&client, id).unwrap();
        let mut b = KcpStream::new(&server, id).unwrap();
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
        a.read_to_end(&mut Vec::new()).await.unwrap();
        b.read_to_end(&mut Vec::new()).await.unwrap();
        timeout(Duration::from_secs(1), async {
            while !client.data.state_map.get(&id).unwrap().fin_acked
                || !server.data.state_map.get(&id).unwrap().fin_acked
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::advance(Duration::from_secs(10)).await;
        tokio::task::yield_now().await;
        let until = client
            .data
            .state_map
            .get(&id)
            .unwrap()
            .closed_until
            .unwrap();
        assert!(!client.data.conn_map.contains_key(&id));
        let mut late = KcpPacket::new(0);
        id.fill_packet_header(&mut late);
        late.mut_header().set_version(1).set_ping(true);
        client.input_sender().send(late.clone()).await.unwrap();
        late.mut_header().set_ping(false).set_fin(true);
        client.input_sender().send(late).await.unwrap();
        tokio::task::yield_now().await;
        assert_eq!(
            client.data.state_map.get(&id).unwrap().closed_until,
            Some(until)
        );
        assert!(!server.data.state_map.get(&id).unwrap().reset);

        client.data.cur_conv.store(id.conv, Ordering::Relaxed);
        let replacement = connect_test_pair(&client, &server).await;
        assert_ne!(replacement, id);
        tokio::time::advance(CLOSE_RETENTION).await;
        tokio::task::yield_now().await;
        assert!(!client.data.state_map.contains_key(&id));
        assert!(!server.data.state_map.contains_key(&id));
        assert!(client.data.state_map.contains_key(&replacement));
    }

    #[tokio::test]
    async fn test_kcp_connect_and_close() {
        let mut p = KcpPacket::new(0);
        let _ = p.mut_header().conv();

        let (client_endpoint, server_endpoint, t) = prepare_test().await;

        let (connect_ret, accept_ret) = tokio::join!(
            client_endpoint.connect(std::time::Duration::from_secs(1), 1, 3, Bytes::from("conn")),
            server_endpoint.accept()
        );

        assert_eq!(*connect_ret.as_ref().unwrap(), accept_ret.unwrap());

        let conv = connect_ret.unwrap();

        let client_conn_data = client_endpoint.conn_data(&conv).unwrap();
        assert_eq!("conn", String::from_utf8_lossy(&client_conn_data));

        let server_conn_data = server_endpoint.conn_data(&conv).unwrap();
        assert_eq!("conn", String::from_utf8_lossy(&server_conn_data));

        let (client_sender, mut client_receiver) =
            client_endpoint.conn_sender_receiver(conv).unwrap();
        let (server_sender, mut server_receiver) =
            server_endpoint.conn_sender_receiver(conv).unwrap();

        client_sender.send(BytesMut::from("hello")).await.unwrap();
        let data = server_receiver.recv().await.unwrap();
        assert_eq!("hello", String::from_utf8_lossy(&data));

        server_sender.send(BytesMut::from("world")).await.unwrap();
        let data = client_receiver.recv().await.unwrap();
        assert_eq!("world", String::from_utf8_lossy(&data));

        // test half close
        drop(client_sender);
        assert!(server_receiver.recv().await.is_none());
        // server can still send data
        server_sender.send(BytesMut::from("world")).await.unwrap();
        let data = client_receiver.recv().await.unwrap();
        assert_eq!("world", String::from_utf8_lossy(&data));

        // full close
        drop(server_sender);
        assert!(client_receiver.recv().await.is_none());

        drop(client_endpoint);
        drop(server_endpoint);

        t.join_all().await;
    }

    async fn connect_test_pair(client: &KcpEndpoint, server: &KcpEndpoint) -> ConnId {
        let (connected, accepted) = timeout(std::time::Duration::from_secs(5), async {
            tokio::try_join!(
                async {
                    client
                        .connect(std::time::Duration::from_secs(1), 1, 2, Bytes::new())
                        .await
                        .context("client handshake failed")
                },
                async { server.accept().await.context("server accept failed") },
            )
        })
        .await
        .expect("test handshake stalled before data transfer")
        .expect("test handshake failed");
        assert_eq!(accepted, connected);
        connected
    }

    #[tokio::test]
    async fn test_accept_after_burst_exceeds_notification_capacity() {
        let (client, server, _tasks) = prepare_test().await;
        let mut connected = std::collections::HashSet::new();
        for _ in 0..32 {
            connected.insert(
                client
                    .connect(std::time::Duration::from_secs(1), 1, 2, Bytes::new())
                    .await
                    .unwrap(),
            );
        }
        timeout(std::time::Duration::from_secs(1), async {
            while server
                .data
                .state_map
                .iter()
                .filter(|state| state.is_established())
                .count()
                != 32
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for _ in 0..32 {
            let id = timeout(std::time::Duration::from_secs(1), server.accept())
                .await
                .expect("established connection lost by accept")
                .unwrap();
            assert!(connected.remove(&id), "connection accepted twice");
        }
        assert!(connected.is_empty());
        assert!(
            timeout(std::time::Duration::from_millis(20), client.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_stream_half_close_drains_buffered_data_and_response() {
        use crate::stream::KcpStream;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (client, server, _tasks) = prepare_test().await;
        let id = connect_test_pair(&client, &server).await;
        let mut client_stream = KcpStream::new(&client, id).unwrap();
        let mut server_stream = KcpStream::new(&server, id).unwrap();
        timeout(std::time::Duration::from_secs(10), async {
            let expected = vec![42; 200 * 1024];
            for chunk in expected.chunks(1024) {
                client_stream.write_all(chunk).await.unwrap();
            }
            client_stream.shutdown().await.unwrap();
            while !server.data.state_map.get(&id).unwrap().is_peer_closed() {
                tokio::task::yield_now().await;
            }
            let mut request = Vec::new();
            server_stream.read_to_end(&mut request).await.unwrap();
            assert_eq!(request, expected);
            server_stream
                .write_all(b"response after EOF")
                .await
                .unwrap();
            server_stream.shutdown().await.unwrap();
            let mut response = Vec::new();
            client_stream.read_to_end(&mut response).await.unwrap();
            assert_eq!(response, b"response after EOF");
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_simultaneous_shutdown_drains_both_directions() {
        use crate::stream::KcpStream;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (client, server, _tasks) = prepare_test().await;
        let id = connect_test_pair(&client, &server).await;
        let mut client_stream = KcpStream::new(&client, id).unwrap();
        let mut server_stream = KcpStream::new(&server, id).unwrap();
        timeout(std::time::Duration::from_secs(15), async {
            for _ in 0..200 {
                client_stream.write_all(&[1; 1024]).await.unwrap();
                server_stream.write_all(&[2; 1024]).await.unwrap();
            }
            client_stream.shutdown().await.unwrap();
            server_stream.shutdown().await.unwrap();
            while !client.data.state_map.get(&id).unwrap().is_closed()
                || !server.data.state_map.get(&id).unwrap().is_closed()
            {
                tokio::task::yield_now().await;
            }
            // Let periodic cleanup run while both application receive queues
            // are still full and the connection has reached Closed.
            tokio::time::sleep(std::time::Duration::from_secs(11)).await;
            let mut client_response = Vec::new();
            let mut server_request = Vec::new();
            client_stream
                .read_to_end(&mut client_response)
                .await
                .unwrap();
            server_stream
                .read_to_end(&mut server_request)
                .await
                .unwrap();
            assert_eq!(client_response, vec![2; 200 * 1024]);
            assert_eq!(server_request, vec![1; 200 * 1024]);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_half_closed_connection_answers_ping() {
        let (client, server, _tasks) = prepare_test().await;
        let id = connect_test_pair(&client, &server).await;
        let (sender, _receiver) = client.conn_sender_receiver(id).unwrap();
        drop(sender);
        timeout(std::time::Duration::from_secs(1), async {
            while !matches!(
                client.data.state_map.get(&id).unwrap().fsm,
                KcpConnectionFSM::LocalClosed
            ) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut ping = KcpPacket::new(0);
        id.fill_packet_header(&mut ping);
        ping.mut_header().set_ping(true).set_version(1);
        let (output, mut received) = tokio::sync::mpsc::channel(1);
        assert!(KcpEndpoint::try_handle_pingpong(&client.data, &ping, &output).await);
        let pong = received.recv().await.unwrap();
        assert!(pong.header().is_pong());
        assert!(!pong.header().is_rst());
    }

    #[tokio::test]
    async fn test_reset_and_endpoint_drop_are_read_errors() {
        use crate::stream::KcpStream;
        use tokio::io::AsyncReadExt;
        for reset in [false, true] {
            let (client, server, _tasks) = prepare_test().await;
            let id = connect_test_pair(&client, &server).await;
            let mut stream = KcpStream::new(&server, id).unwrap();
            let reader =
                tokio::spawn(async move { stream.read(&mut [0; 1]).await.unwrap_err().kind() });
            tokio::task::yield_now().await;
            if reset {
                let mut packet = KcpPacket::new(0);
                id.fill_packet_header(&mut packet);
                packet.mut_header().set_rst(true).set_version(1);
                server.input_sender().send(packet).await.unwrap();
            } else {
                drop(server);
            }
            let error = timeout(std::time::Duration::from_secs(1), reader)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(error, std::io::ErrorKind::BrokenPipe);
        }
    }
    #[tokio::test]
    async fn test_accept_after_empty_request_half_close() {
        use crate::stream::KcpStream;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (client, server, _tasks) = prepare_test().await;
        let id = client
            .connect(std::time::Duration::from_secs(1), 1, 2, Bytes::new())
            .await
            .unwrap();
        let mut client_stream = KcpStream::new(&client, id).unwrap();
        client_stream.shutdown().await.unwrap();
        timeout(std::time::Duration::from_secs(1), async {
            while !server.data.state_map.get(&id).unwrap().is_peer_closed() {
                tokio::task::yield_now().await;
            }
            assert_eq!(server.accept().await.unwrap(), id);
            let mut server_stream = KcpStream::new(&server, id).unwrap();
            assert_eq!(server_stream.read(&mut [0; 1]).await.unwrap(), 0);
            server_stream
                .write_all(b"empty request response")
                .await
                .unwrap();
            server_stream.shutdown().await.unwrap();
            let mut response = Vec::new();
            client_stream.read_to_end(&mut response).await.unwrap();
            assert_eq!(response, b"empty request response");
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_connect_after_peer_half_close() {
        use crate::stream::KcpStream;
        use tokio::io::AsyncReadExt;
        let mut client = KcpEndpoint::new();
        client.run().await;
        let mut output = client.output_receiver().unwrap();
        let mut connecting =
            Box::pin(client.connect(std::time::Duration::from_secs(1), 1, 2, Bytes::new()));
        let syn = tokio::select! {
            _ = &mut connecting => panic!("connect completed before SYN-ACK"),
            packet = output.recv() => packet.unwrap(),
        };
        let id = ConnId::from(&syn);
        let mut syn_ack = KcpPacket::new(0);
        id.fill_packet_header(&mut syn_ack);
        syn_ack.mut_header().set_syn(true).set_ack(true);
        let mut fin = KcpPacket::new(0);
        id.fill_packet_header(&mut fin);
        fin.mut_header().set_fin(true);
        client.input_sender().send(syn_ack).await.unwrap();
        client.input_sender().send(fin).await.unwrap();
        timeout(std::time::Duration::from_secs(1), async {
            // Hold the connect future after its handshake notification until
            // FIN has also arrived, modelling an immediately empty response.
            while !client.data.state_map.get(&id).unwrap().is_peer_closed() {
                tokio::task::yield_now().await;
            }
            assert_eq!(connecting.await.unwrap(), id);
            let mut stream = KcpStream::new(&client, id).unwrap();
            assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn test_keepalive_waits_for_completed_handshake() {
        let mut endpoint = KcpEndpoint::new();
        let mut expected = std::collections::HashSet::new();
        for (conv, fsm) in [
            KcpConnectionFSM::Listen,
            KcpConnectionFSM::SynSent,
            KcpConnectionFSM::SynReceived,
            KcpConnectionFSM::Established,
            KcpConnectionFSM::LocalClosed,
            KcpConnectionFSM::PeerClosed,
            KcpConnectionFSM::Closed,
        ]
        .into_iter()
        .enumerate()
        {
            let id = ConnId {
                conv: conv as u32,
                src_session_id: 1,
                dst_session_id: 2,
            };
            if (3..6).contains(&conv) {
                expected.insert(id);
            }
            endpoint
                .data
                .state_map
                .insert(id, KcpConnectionState::new(fsm));
        }
        endpoint.run().await;
        let mut output = endpoint.output_receiver().unwrap();
        while let Ok(Some(packet)) =
            timeout(std::time::Duration::from_millis(50), output.recv()).await
        {
            assert!(packet.header().is_ping());
            assert!(
                expected.remove(&ConnId::from(&packet)),
                "keepalive sent before handshake completed"
            );
        }
        assert!(
            expected.is_empty(),
            "established or half-closed connection lost keepalive"
        );
    }
}
