use crate::{Error, PairedReceiver, Result, VideoFormat, connection::Transport, crypto, video::VideoPackets};
use plist::{Dictionary, Value};
use std::{
    collections::BTreeMap,
    io::Write,
    net::{Shutdown, TcpStream},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// One receiver's live transport. The host owns capture, encoding and source identity.
/// Independent instances can target different receivers; the host must reserve each TV.
pub struct MirrorSession {
    receiver: PairedReceiver,
    video: TcpStream,
    events: EventWorker,
    packets: VideoPackets,
    clock: Clock,
    feedback_at: Instant,
    active: bool,
    configured: bool,
}
struct EventWorker {
    socket: TcpStream,
    worker: Option<JoinHandle<()>>,
    failures: mpsc::Receiver<Error>,
}
impl Drop for EventWorker {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
struct Clock {
    timeline: u64,
    milliseconds: u64,
    sampled: Instant,
}
impl Clock {
    fn anchor(&mut self, headers: &BTreeMap<String, String>) -> Result<()> {
        let number = |name| {
            headers
                .get(name)
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or(Error::Protocol("missing modern receiver clock"))
        };
        self.milliseconds = number("x-apple-requestreceivedtimestamp")?
            .checked_add(number("x-apple-processingtime")?)
            .ok_or(Error::Protocol("receiver clock overflow"))?;
        self.sampled = Instant::now();
        Ok(())
    }
    fn timestamp(&self) -> Result<u64> {
        let nanos = u128::from(self.milliseconds) * 1_000_000 + self.sampled.elapsed().as_nanos() + 75_000_000;
        u64::try_from((nanos << 32) / 1_000_000_000).map_err(|_| Error::Protocol("receiver timestamp overflow"))
    }
}

fn dictionary(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Dictionary(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}
fn encoded(value: &Value) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    value.to_writer_binary(&mut body)?;
    Ok(body)
}
fn decoded(bytes: &[u8]) -> Result<Value> {
    Ok(Value::from_reader(std::io::Cursor::new(bytes))?)
}
fn integer(dictionary: &Dictionary, key: &'static str) -> Result<u64> {
    dictionary
        .get(key)
        .and_then(Value::as_unsigned_integer)
        .ok_or(Error::ReceiverInteger(key))
}
fn clock_id(dictionary: &Dictionary) -> Result<u64> {
    let value = dictionary.get("ClockID").ok_or(Error::ReceiverInteger("ClockID"))?;
    // Clock identities are 64-bit bit patterns, including when a plist decodes them as signed.
    value
        .as_unsigned_integer()
        .or_else(|| {
            value
                .as_signed_integer()
                .map(|value| u64::from_be_bytes(value.to_be_bytes()))
        })
        .filter(|value| *value != 0)
        .ok_or(Error::ReceiverInteger("ClockID"))
}
fn port(dictionary: &Dictionary, key: &'static str) -> Result<u16> {
    let value = u16::try_from(integer(dictionary, key)?).map_err(|_| Error::Protocol("invalid receiver port"))?;
    if value == 0 {
        return Err(Error::Protocol("zero receiver port"));
    }
    Ok(value)
}

impl PairedReceiver {
    /// Start a modern PTP-timed, encrypted H.264 mirroring session.
    ///
    /// # Errors
    /// Rejects unsupported receivers, missing timing and rejected setup; no fallback.
    pub fn mirror(mut self, format: VideoFormat) -> Result<MirrorSession> {
        let info = self.info()?;
        let info = info.as_dictionary().ok_or(Error::Protocol("invalid receiver info"))?;
        let model = info
            .get("model")
            .and_then(Value::as_string)
            .ok_or(Error::Protocol("missing receiver model"))?;
        if !model.starts_with("AppleTV") || integer(info, "features")? & (1 << 41) == 0 {
            return Err(Error::Protocol("receiver lacks modern Apple TV timing"));
        }
        let peer = dictionary([
            ("ID", uuid::Uuid::new_v4().to_string().into()),
            ("DeviceType", 0u64.into()),
            ("SupportsClockPortMatchingOverride", true.into()),
            ("Addresses", Value::Array(vec![self.transport.local_ip()?.into()])),
        ]);
        let mut device = *crypto::random::<6>()?;
        device[0] = (device[0] | 2) & 0xfe;
        let device_id = device
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        let control = dictionary([
            ("sessionUUID", self.identity.clone().into()),
            ("deviceID", device_id.into()),
            ("name", "Horizon".into()),
            ("model", "Linux".into()),
            ("sourceVersion", "890.1".into()),
            ("isScreenMirroringSession", true.into()),
            ("timingProtocol", "PTP".into()),
            ("timingPeerInfo", peer.clone()),
            ("timingPeerList", Value::Array(vec![peer])),
            ("combinedGetInfoWithControlSetup", true.into()),
            ("updateSessionRequest", false.into()),
        ]);
        let response = self.transport.request(
            "SETUP",
            "/stream",
            "Content-Type: application/x-apple-binary-plist\r\n",
            &encoded(&control)?,
        )?;
        let setup = decoded(&response.body)?;
        let setup = setup.as_dictionary().ok_or(Error::Protocol("invalid control setup"))?;
        let clock_info = setup
            .get("timingPeerInfo")
            .and_then(Value::as_dictionary)
            .ok_or(Error::Protocol("missing PTP peer"))?;
        let mut clock = Clock {
            timeline: clock_id(clock_info)?,
            milliseconds: 0,
            sampled: Instant::now(),
        };
        clock.anchor(&response.headers)?;
        let event_port = port(setup, "eventPort")?;
        let mut event_transport = Transport::connect_cancellable(
            std::net::SocketAddr::new(self.address.ip(), event_port),
            self.transport.cancellation(),
        )?;
        event_transport.encrypt(&self.shared, true)?;
        event_transport.wait_for_events()?;
        let events = event_transport.shutdown_handle()?;
        let (send, failures) = mpsc::channel();
        let timeline = clock.timeline;
        let worker = thread::spawn(move || {
            if let Err(error) = events_loop(&mut event_transport, timeline) {
                let _ = send.send(error);
            }
        });
        // An event worker started before video SETUP must be stopped on every exit.
        self.finish_mirror(
            format,
            clock,
            EventWorker {
                socket: events,
                worker: Some(worker),
                failures,
            },
        )
    }
    fn finish_mirror(mut self, format: VideoFormat, clock: Clock, events: EventWorker) -> Result<MirrorSession> {
        let stream = u64::from_le_bytes(*crypto::random::<8>()?) & i64::MAX as u64;
        let video_setup = dictionary([(
            "streams",
            Value::Array(vec![dictionary([
                ("type", 110u64.into()),
                ("streamConnectionID", stream.into()),
                ("latencyMs", 75u64.into()),
                ("shk", Value::Data(crypto::random::<16>()?.to_vec())),
                ("shiv", Value::Data(crypto::random::<16>()?.to_vec())),
            ])]),
        )]);
        let connect = (|| {
            let uri = format!("rtsp://{}/{stream}", self.address);
            let response = self.transport.request(
                "SETUP",
                &uri,
                "Content-Type: application/x-apple-binary-plist\r\n",
                &encoded(&video_setup)?,
            )?;
            let value = decoded(&response.body)?;
            let streams = value
                .as_dictionary()
                .and_then(|v| v.get("streams"))
                .and_then(Value::as_array)
                .ok_or(Error::Protocol("missing video streams"))?;
            let stream = streams
                .iter()
                .filter_map(Value::as_dictionary)
                .find(|v| integer(v, "type").ok() == Some(110))
                .ok_or(Error::Protocol("video stream missing"))?;
            let endpoint = std::net::SocketAddr::new(self.address.ip(), port(stream, "dataPort")?);
            let cancellation = self.transport.cancellation();
            let video = match &cancellation {
                Some(cancel) => cancel.connect(endpoint)?,
                None => TcpStream::connect_timeout(&endpoint, Duration::from_secs(5))?,
            };
            let _registration = cancellation.map(|cancel| cancel.register(&video)).transpose()?;
            video.set_nodelay(true)?;
            video.set_write_timeout(Some(Duration::from_secs(2)))?;
            self.transport.request("RECORD", "/stream", "", &[])?;
            Ok::<_, Error>(video)
        })();
        match connect {
            Ok(video) => {
                self.transport.complete_setup()?;
                Ok(MirrorSession {
                    packets: VideoPackets::new(&self.shared, stream, format.dimensions())?,
                    receiver: self,
                    video,
                    events,

                    clock,
                    feedback_at: Instant::now(),
                    active: true,
                    configured: false,
                })
            }
            Err(error) => Err(error),
        }
    }
}

fn events_loop(transport: &mut Transport, timeline: u64) -> Result<()> {
    loop {
        let (request, response) = transport.message()?;
        let sequence = response
            .headers
            .get("cseq")
            .ok_or(Error::Protocol("event missing CSeq"))?;
        let sequence = sequence
            .parse::<u32>()
            .map_err(|_| Error::Protocol("invalid event CSeq"))?;
        if !request.ends_with(" RTSP/1.0") {
            return Err(Error::Protocol("invalid event request"));
        }
        if !response.body.is_empty() {
            let value = decoded(&response.body)?;
            if let Some(peer) = value
                .as_dictionary()
                .and_then(|v| v.get("timingPeerInfo"))
                .and_then(Value::as_dictionary)
                && clock_id(peer)? != timeline
            {
                return Err(Error::Protocol("receiver timing changed; restart casting"));
            }
        }
        transport.send(format!("RTSP/1.0 200 OK\r\nCSeq: {sequence}\r\nContent-Length: 0\r\n\r\n").as_bytes())?;
    }
}
impl MirrorSession {
    /// Supply raw H.264 SPS and PPS NAL units before sending any access units.
    /// # Errors
    /// Fails for invalid parameter sets or a closed transport.
    pub fn configure(&mut self, sps: &[u8], pps: &[u8]) -> Result<()> {
        let packet = self.packets.configuration(sps, pps)?;
        self.write(&packet)?;
        self.configured = true;
        Ok(())
    }
    /// Send one complete encoder access unit. NALs exclude Annex-B start codes.
    /// # Errors
    /// Fails on malformed NALs, receiver loss or timing changes.
    pub fn send(&mut self, nals: &[&[u8]]) -> Result<()> {
        if !self.active || !self.configured {
            return Err(Error::Protocol("session is not configured"));
        }
        let result = self.send_active(nals);
        if result.is_err() {
            self.stop();
        }
        result
    }
    fn send_active(&mut self, nals: &[&[u8]]) -> Result<()> {
        if self.feedback_at.elapsed() >= Duration::from_secs(2) {
            let response = self.receiver.transport.request("POST", "/feedback", "", &[])?;
            self.clock.anchor(&response.headers)?;
            self.feedback_at = Instant::now();
        }
        let packet = self.packets.frame(nals, self.clock.timestamp()?, self.clock.timeline)?;
        self.write(&packet)
    }
    fn write(&mut self, packet: &[u8]) -> Result<()> {
        if !self.active {
            return Err(Error::Protocol("session stopped"));
        }
        if let Ok(error) = self.events.failures.try_recv() {
            self.stop();
            return Err(error);
        }
        if let Err(error) = self.video.write_all(packet) {
            self.stop();
            return Err(error.into());
        }
        Ok(())
    }
    /// Stop this receiver only. Repeated calls are harmless.
    pub fn stop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let _ = self.video.shutdown(Shutdown::Both);
        let _ = self.events.socket.shutdown(Shutdown::Both);
        if let Some(worker) = self.events.worker.take() {
            let _ = worker.join();
        }
        let _ = self.receiver.transport.request("TEARDOWN", "/stream", "", &[]);
    }
}
impl Drop for MirrorSession {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_identity_preserves_all_bits_without_relaxing_ports() {
        let bits = 0xf123_4567_89ab_cdef_u64;
        for value in [Value::from(bits), Value::from(i64::from_be_bytes(bits.to_be_bytes()))] {
            let wire = encoded(&dictionary([("ClockID", value)])).expect("fixture");
            let value = decoded(&wire).expect("plist");
            assert_eq!(
                clock_id(value.as_dictionary().expect("dictionary")).expect("clock"),
                bits
            );
        }
        for value in [Value::from(0u64), Value::Real(123.0), Value::from("123")] {
            let peer = dictionary([("ClockID", value)]);
            assert!(clock_id(peer.as_dictionary().expect("dictionary")).is_err());
        }
        let peer = dictionary([("eventPort", Value::from(-1i64))]);
        assert!(port(peer.as_dictionary().expect("dictionary"), "eventPort").is_err());
    }
}
