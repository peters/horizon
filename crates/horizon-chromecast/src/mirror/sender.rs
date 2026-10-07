//! The UDP side of a Cast Streaming session. It encrypts and packetizes
//! frames, keeps the last second of them for retransmission, sends sender
//! reports, and acts on the receiver's feedback.
use super::{
    crypto::FrameCipher,
    rtcp::{self, Feedback, SenderReport},
    rtp::{self, Frame},
};
use std::{
    collections::VecDeque,
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const REPORT_INTERVAL: Duration = Duration::from_millis(500);
const RECEIVE_POLL: Duration = Duration::from_millis(20);
/// A packet just resent is not resent again for a loss report that went out
/// before the resend could arrive.
const RESEND_GAP: Duration = Duration::from_millis(50);
/// Receivers repeat a picture loss report until a key frame arrives; reports
/// within this time of the last key frame ask for that same key frame.
const KEYFRAME_GRACE: Duration = Duration::from_millis(300);
/// A full send buffer is retried this often before the packet is dropped
/// (the receiver then asks for it again).
const SEND_RETRIES: u32 = 40;
const SEND_RETRY: Duration = Duration::from_micros(250);

/// One stream the receiver accepted.
pub(crate) struct StreamSetup {
    pub ssrc: u32,
    pub payload_type: u8,
    pub clock_rate: u32,
    pub cipher: FrameCipher,
    /// Audio frames all decode on their own.
    pub independent_frames: bool,
}

struct SentFrame {
    id: u32,
    at: Instant,
    packets: Arc<Vec<Vec<u8>>>,
    resent: Vec<Option<Instant>>,
}

struct StreamState {
    setup: StreamSetup,
    next_frame: u32,
    sequence: u16,
    rtp_offset: u32,
    packets: u32,
    octets: u32,
    history: VecDeque<SentFrame>,
}

/// Media time `media` corresponds to wall clock instant `at`.
#[derive(Clone, Copy)]
struct Origin {
    at: Instant,
    media: Duration,
}

struct Shared {
    socket: UdpSocket,
    streams: Mutex<Vec<StreamState>>,
    origin: Mutex<Option<Origin>>,
    /// How long sent frames stay available for resending.
    history: Duration,
    keyframe_wanted: AtomicBool,
    last_keyframe: Mutex<Option<Instant>>,
    /// The receiver's last report on the video stream.
    last_feedback: Mutex<Option<Instant>>,
    stop: AtomicBool,
}

pub(crate) struct Sender {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// Which accepted stream a frame belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Track {
    Video,
    Audio,
}

impl Sender {
    /// Starts sending to `target`. `streams` holds the video stream first,
    /// then the audio stream if the receiver accepted one. Frames stay
    /// available for resending for `history`.
    /// # Errors
    /// Fails when no UDP socket can be opened towards `target`.
    pub(crate) fn start(
        target: SocketAddr,
        streams: Vec<StreamSetup>,
        rtp_offsets: &[u32],
        history: Duration,
    ) -> std::io::Result<Self> {
        let local: SocketAddr = if target.is_ipv4() {
            ([0, 0, 0, 0], 0).into()
        } else {
            (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
        };
        let socket = UdpSocket::bind(local)?;
        socket.connect(target)?;
        socket.set_read_timeout(Some(RECEIVE_POLL))?;
        let streams = streams
            .into_iter()
            .zip(rtp_offsets.iter().copied().chain(std::iter::repeat(0)))
            .map(|(setup, rtp_offset)| StreamState {
                setup,
                next_frame: 0,
                sequence: 0,
                rtp_offset,
                packets: 0,
                octets: 0,
                history: VecDeque::new(),
            })
            .collect();
        let shared = Arc::new(Shared {
            socket,
            streams: Mutex::new(streams),
            origin: Mutex::new(None),
            history,
            keyframe_wanted: AtomicBool::new(true),
            last_keyframe: Mutex::new(None),
            last_feedback: Mutex::new(None),
            stop: AtomicBool::new(false),
        });
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("chromecast-mirror".to_owned())
                .spawn(move || shared.run())?
        };
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// Sends one Annex B access unit. The receiver can only start on a key
    /// frame, so delta frames before the first one are dropped.
    pub(crate) fn send_video(&self, annexb: &[u8], pts: Duration, keyframe: bool) {
        if keyframe {
            self.shared.keyframe_wanted.store(false, Ordering::Release);
        }
        self.shared.send(Track::Video, annexb, pts, keyframe);
    }

    /// Sends one raw AAC frame, if the receiver accepted an audio stream.
    pub(crate) fn send_audio(&self, frame: &[u8], pts: Duration) {
        self.shared.send(Track::Audio, frame, pts, true);
    }

    /// The receiver lost a picture, or has none yet.
    pub(crate) fn wants_keyframe(&self) -> bool {
        self.shared.keyframe_wanted.load(Ordering::Acquire)
    }

    /// When the receiver last reported on the video stream.
    pub(crate) fn last_feedback(&self) -> Option<Instant> {
        *lock(&self.shared.last_feedback)
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Shared {
    /// Encrypts and packetizes under the lock, then sends without it, so the
    /// encoder, the audio and the feedback threads never wait on the network.
    fn send(&self, track: Track, payload: &[u8], pts: Duration, keyframe: bool) {
        let index = match track {
            Track::Video => 0,
            Track::Audio => 1,
        };
        let packets = {
            let mut streams = lock(&self.streams);
            let Some(stream) = streams.get_mut(index) else {
                return;
            };
            if stream.next_frame == 0 && !keyframe {
                return;
            }
            lock(&self.origin).get_or_insert(Origin {
                at: Instant::now(),
                media: pts,
            });
            let id = stream.next_frame;
            stream.next_frame += 1;
            let mut data = payload.to_vec();
            stream.setup.cipher.apply(id, &mut data);
            let independent = keyframe || stream.setup.independent_frames;
            let frame = Frame {
                id,
                referenced: if independent { id } else { id.wrapping_sub(1) },
                keyframe: independent,
                rtp_timestamp: stream.rtp_offset.wrapping_add(ticks(pts, stream.setup.clock_rate)),
                playout_delay_ms: None,
                payload: &data,
            };
            let packets = Arc::new(rtp::packetize(
                &frame,
                stream.setup.payload_type,
                stream.setup.ssrc,
                &mut stream.sequence,
            ));
            for packet in packets.iter() {
                stream.packets = stream.packets.wrapping_add(1);
                stream.octets = stream
                    .octets
                    .wrapping_add(u32::try_from(packet.len()).unwrap_or(u32::MAX));
            }
            let now = Instant::now();
            while stream
                .history
                .front()
                .is_some_and(|old| now.duration_since(old.at) > self.history || stream.history.len() >= 255)
            {
                stream.history.pop_front();
            }
            stream.history.push_back(SentFrame {
                id,
                at: now,
                packets: packets.clone(),
                resent: vec![None; packets.len()],
            });
            packets
        };
        if keyframe && track == Track::Video {
            *lock(&self.last_keyframe) = Some(Instant::now());
        }
        for packet in packets.iter() {
            self.transmit(packet);
        }
    }

    fn transmit(&self, packet: &[u8]) {
        for _ in 0..SEND_RETRIES {
            match self.socket.send(packet) {
                // The interface queue is full during a key frame burst: wait it out.
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock || error.raw_os_error() == Some(NO_BUFFER_SPACE) =>
                {
                    std::thread::sleep(SEND_RETRY);
                }
                _ => return,
            }
        }
    }

    fn run(&self) {
        let mut buffer = [0u8; 1500];
        let mut next_report = Instant::now();
        while !self.stop.load(Ordering::Acquire) {
            match self.socket.recv(&mut buffer) {
                Ok(read) if rtcp::is_rtcp(&buffer[..read]) => {
                    for feedback in rtcp::parse(&buffer[..read]) {
                        self.apply(&feedback);
                    }
                }
                _ => {}
            }
            let now = Instant::now();
            if now >= next_report {
                // Until the first frame there is no clock to report: try again
                // on the next pass, so the first report follows it closely.
                next_report = if self.report(now) { now + REPORT_INTERVAL } else { now };
            }
        }
    }

    fn apply(&self, feedback: &Feedback) {
        match feedback {
            Feedback::PictureLoss { media_ssrc } => {
                let video = lock(&self.streams)
                    .first()
                    .is_some_and(|video| video.setup.ssrc == *media_ssrc);
                let underway = lock(&self.last_keyframe).is_some_and(|at| at.elapsed() < KEYFRAME_GRACE);
                if video && !underway {
                    self.keyframe_wanted.store(true, Ordering::Release);
                }
            }
            Feedback::Cast { media_ssrc, losses, .. } => {
                let mut resend = Vec::new();
                {
                    let mut streams = lock(&self.streams);
                    let Some((index, stream)) = streams
                        .iter_mut()
                        .enumerate()
                        .find(|(_, stream)| stream.setup.ssrc == *media_ssrc)
                    else {
                        return;
                    };
                    let now = Instant::now();
                    if index == 0 {
                        *lock(&self.last_feedback) = Some(now);
                    }
                    for loss in losses {
                        let Some(frame) = stream
                            .history
                            .iter_mut()
                            .rev()
                            .find(|frame| frame.id.to_be_bytes()[3] == loss.frame)
                        else {
                            continue;
                        };
                        let last = u16::try_from(frame.packets.len() - 1).unwrap_or(u16::MAX);
                        for packet in loss.packets(last) {
                            let packet = usize::from(packet);
                            if frame.resent[packet].is_some_and(|at| now.duration_since(at) < RESEND_GAP) {
                                continue;
                            }
                            frame.resent[packet] = Some(now);
                            resend.push((frame.packets.clone(), packet));
                        }
                    }
                }
                for (packets, packet) in resend {
                    self.transmit(&packets[packet]);
                }
            }
        }
    }

    /// Maps the current media time to wall clock time for every stream that
    /// has sent. False before the first frame.
    fn report(&self, now: Instant) -> bool {
        let Some(origin) = *lock(&self.origin) else {
            return false;
        };
        let media = origin.media + now.saturating_duration_since(origin.at);
        let ntp = rtcp::ntp_now();
        let reports: Vec<[u8; 28]> = lock(&self.streams)
            .iter()
            .filter(|stream| stream.next_frame > 0)
            .map(|stream| {
                SenderReport {
                    ssrc: stream.setup.ssrc,
                    ntp,
                    rtp_timestamp: stream.rtp_offset.wrapping_add(ticks(media, stream.setup.clock_rate)),
                    packets: stream.packets,
                    octets: stream.octets,
                }
                .encode()
            })
            .collect();
        for report in &reports {
            self.transmit(report);
        }
        true
    }
}

/// `ENOBUFS`: Apple platforms report a full UDP send queue this way.
#[cfg(any(target_os = "macos", target_os = "ios"))]
const NO_BUFFER_SPACE: i32 = 55;
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
const NO_BUFFER_SPACE: i32 = 105;

/// RTP timestamp units in `time`, wrapping at 32 bits.
fn ticks(time: Duration, clock_rate: u32) -> u32 {
    let ticks = time.as_nanos() * u128::from(clock_rate) / 1_000_000_000;
    u32::try_from(ticks % (1 << 32)).unwrap_or_default()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 16] = [7; 16];
    const IV_MASK: [u8; 16] = [3; 16];
    const SSRC: u32 = 0x1234;

    fn video() -> StreamSetup {
        StreamSetup {
            ssrc: SSRC,
            payload_type: 96,
            clock_rate: 90_000,
            cipher: FrameCipher::new(KEY, IV_MASK),
            independent_frames: false,
        }
    }

    fn receiver() -> UdpSocket {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        socket
    }

    /// The next RTP packet, skipping sender reports.
    fn next_rtp(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
        let mut buffer = [0u8; 2048];
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            assert!(Instant::now() < deadline, "no RTP packet");
            let (read, from) = socket.recv_from(&mut buffer).unwrap();
            if !rtcp::is_rtcp(&buffer[..read]) {
                return (buffer[..read].to_vec(), from);
            }
        }
    }

    fn cast_feedback(frame: u8, packet: u16) -> Vec<u8> {
        let mut out = vec![0x8f, 206, 0, 5, 0, 0, 0, 1];
        out.extend_from_slice(&SSRC.to_be_bytes());
        out.extend_from_slice(b"CAST");
        out.extend_from_slice(&[frame.wrapping_sub(1), 1, 0, 100, frame]);
        out.extend_from_slice(&packet.to_be_bytes());
        out.push(0);
        out
    }

    #[test]
    fn frames_arrive_encrypted_in_packets_and_lost_packets_are_resent() {
        let socket = receiver();
        let sender = Sender::start(
            socket.local_addr().unwrap(),
            vec![video()],
            &[1000],
            Duration::from_secs(1),
        )
        .unwrap();
        // Delta frames before the first key frame cannot be decoded: dropped.
        sender.send_video(&[9; 50], Duration::ZERO, false);
        assert!(sender.wants_keyframe());
        let frame: Vec<u8> = (0..3000u32).map(|n| n.to_le_bytes()[1]).collect();
        sender.send_video(&frame, Duration::from_millis(100), true);
        assert!(!sender.wants_keyframe());
        let mut packets = Vec::new();
        let mut from = None;
        for _ in 0..3 {
            let (packet, source) = next_rtp(&socket);
            from = Some(source);
            packets.push(packet);
        }
        for (index, packet) in packets.iter().enumerate() {
            assert_eq!(packet[12] & 0x80, 0x80, "key frame");
            assert_eq!(packet[13], 0, "frame 0: the dropped delta frame took no ID");
            assert_eq!(&packet[14..16], &u16::try_from(index).unwrap().to_be_bytes());
            assert_eq!(&packet[16..18], &[0, 2]);
            // 100 ms at 90 kHz after the RTP offset.
            assert_eq!(&packet[4..8], &(1000u32 + 9000).to_be_bytes());
        }
        let mut payload: Vec<u8> = packets.iter().flat_map(|packet| packet[19..].to_vec()).collect();
        FrameCipher::new(KEY, IV_MASK).apply(0, &mut payload);
        assert_eq!(payload, frame);
        socket.send_to(&cast_feedback(0, 1), from.unwrap()).unwrap();
        let (resent, _) = next_rtp(&socket);
        assert_eq!(resent, packets[1]);
        assert!(sender.last_feedback().is_some());
    }

    #[test]
    fn picture_loss_asks_for_a_key_frame_and_reports_map_the_clock() {
        let socket = receiver();
        let sender = Sender::start(
            socket.local_addr().unwrap(),
            vec![video()],
            &[0],
            Duration::from_secs(1),
        )
        .unwrap();
        sender.send_video(&[1, 2, 3], Duration::from_secs(5), true);
        let (_, from) = next_rtp(&socket);
        let mut buffer = [0u8; 2048];
        let report = loop {
            let (read, _) = socket.recv_from(&mut buffer).unwrap();
            if rtcp::is_rtcp(&buffer[..read]) {
                break buffer[..read].to_vec();
            }
        };
        assert_eq!(report[1], 200);
        assert_eq!(&report[4..8], &SSRC.to_be_bytes());
        let reported = u32::from_be_bytes(report[16..20].try_into().unwrap());
        // The report maps "now", just after the 5 s frame, to the RTP clock.
        assert!((5 * 90_000..6 * 90_000).contains(&reported), "{reported}");
        assert!(!sender.wants_keyframe());
        let mut picture_loss = vec![0x81, 206, 0, 2, 0, 0, 0, 1];
        picture_loss.extend_from_slice(&SSRC.to_be_bytes());
        // Right after a key frame, a picture loss report asks for that key frame.
        socket.send_to(&picture_loss, from).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!sender.wants_keyframe());
        std::thread::sleep(KEYFRAME_GRACE);
        socket.send_to(&picture_loss, from).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !sender.wants_keyframe() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(sender.wants_keyframe());
    }
}
