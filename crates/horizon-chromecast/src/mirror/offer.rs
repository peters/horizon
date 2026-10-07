//! Session setup on `urn:x-cast:com.google.cast.webrtc`: the sender OFFERs
//! its streams with their encryption keys, and the receiver ANSWERs with the
//! UDP port to send to and the streams it accepts.
use crate::{Error, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fmt::Write as _, time::Duration};

pub(crate) const NS_WEBRTC: &str = "urn:x-cast:com.google.cast.webrtc";
/// Android TV receivers expect these payload types, whatever the codec.
const VIDEO_PAYLOAD_TYPE: u8 = 96;
const AUDIO_PAYLOAD_TYPE: u8 = 127;
const VIDEO_CLOCK: u32 = 90_000;
const MAX_VIDEO_BIT_RATE: u32 = 10_000_000;
const AUDIO_BIT_RATE: u32 = 128_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Video,
    Audio { sample_rate: u32, channels: u8 },
}

pub(crate) struct StreamOffer {
    pub index: u32,
    pub kind: Kind,
    pub ssrc: u32,
    pub key: [u8; 16],
    pub iv_mask: [u8; 16],
}

impl StreamOffer {
    pub(crate) fn payload_type(&self) -> u8 {
        match self.kind {
            Kind::Video => VIDEO_PAYLOAD_TYPE,
            Kind::Audio { .. } => AUDIO_PAYLOAD_TYPE,
        }
    }

    pub(crate) fn clock_rate(&self) -> u32 {
        match self.kind {
            Kind::Video => VIDEO_CLOCK,
            Kind::Audio { sample_rate, .. } => sample_rate,
        }
    }

    fn describe(&self, target_delay_ms: u64) -> Value {
        let mut stream = json!({
            "index": self.index,
            "rtpProfile": "cast",
            "rtpPayloadType": self.payload_type(),
            "ssrc": self.ssrc,
            "targetDelay": target_delay_ms,
            "aesKey": hex(&self.key),
            "aesIvMask": hex(&self.iv_mask),
            "timeBase": format!("1/{}", self.clock_rate()),
            "receiverRtcpEventLog": false,
            "rtpExtensions": ["adaptive_playout_delay"],
        });
        let details = match self.kind {
            Kind::Video => json!({
                "type": "video_source",
                "codecName": "h264",
                "maxFrameRate": "60000/1000",
                "maxBitRate": MAX_VIDEO_BIT_RATE,
                "resolutions": [{"width": 1920, "height": 1080}],
            }),
            Kind::Audio { sample_rate, channels } => json!({
                "type": "audio_source",
                "codecName": "aac",
                "bitRate": AUDIO_BIT_RATE,
                "sampleRate": sample_rate,
                "channels": channels,
            }),
        };
        if let (Some(stream), Value::Object(details)) = (stream.as_object_mut(), details) {
            stream.extend(details);
        }
        stream
    }
}

/// The OFFER message for `streams`, played `target_delay` behind the sender.
pub(crate) fn offer(seq: u64, streams: &[StreamOffer], target_delay: Duration) -> Value {
    let delay_ms = u64::try_from(target_delay.as_millis()).unwrap_or(u64::MAX);
    let supported: Vec<Value> = streams.iter().map(|stream| stream.describe(delay_ms)).collect();
    json!({
        "type": "OFFER",
        "seqNum": seq,
        "offer": {
            "castMode": "mirroring",
            "receiverGetStatus": false,
            "supportedStreams": supported,
        },
    })
}

/// What the receiver accepted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    pub udp_port: u16,
    /// Accepted stream indexes, each with the SSRC the receiver reports from.
    pub accepted: Vec<(u32, u32)>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnswerBody {
    udp_port: u16,
    #[serde(default)]
    send_indexes: Vec<u32>,
    #[serde(default)]
    ssrcs: Vec<u32>,
}

/// The receiver's reply to OFFER `seq`, or `None` for any other message.
pub(crate) fn answer(payload: &Value, seq: u64) -> Option<Result<Answer>> {
    if payload.get("type").and_then(Value::as_str) != Some("ANSWER")
        || payload.get("seqNum").and_then(Value::as_u64) != Some(seq)
    {
        return None;
    }
    if payload.get("result").and_then(Value::as_str) != Some("ok") {
        let reason = payload
            .pointer("/error/description")
            .and_then(Value::as_str)
            .map(str::to_owned);
        return Some(Err(Error::Rejected {
            kind: "ANSWER".to_owned(),
            reason,
        }));
    }
    let body = payload
        .get("answer")
        .ok_or(Error::Protocol("ANSWER without body"))
        .and_then(|body| Ok(AnswerBody::deserialize(body)?));
    Some(body.map(|body| Answer {
        udp_port: body.udp_port,
        accepted: body.send_indexes.into_iter().zip(body.ssrcs).collect(),
    }))
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_offer_describes_video_and_audio_streams() {
        let streams = [
            StreamOffer {
                index: 0,
                kind: Kind::Video,
                ssrc: 11,
                key: [0xab; 16],
                iv_mask: [0x01; 16],
            },
            StreamOffer {
                index: 1,
                kind: Kind::Audio {
                    sample_rate: 48_000,
                    channels: 2,
                },
                ssrc: 12,
                key: [0; 16],
                iv_mask: [0; 16],
            },
        ];
        let message = offer(3, &streams, Duration::from_millis(120));
        assert_eq!(message["type"], "OFFER");
        assert_eq!(message["seqNum"], 3);
        assert_eq!(message["offer"]["castMode"], "mirroring");
        let video = &message["offer"]["supportedStreams"][0];
        assert_eq!(video["type"], "video_source");
        assert_eq!(video["codecName"], "h264");
        assert_eq!(video["rtpPayloadType"], 96);
        assert_eq!(video["timeBase"], "1/90000");
        assert_eq!(video["targetDelay"], 120);
        assert_eq!(video["aesKey"], "ab".repeat(16));
        assert_eq!(video["aesIvMask"], "01".repeat(16));
        let audio = &message["offer"]["supportedStreams"][1];
        assert_eq!(audio["type"], "audio_source");
        assert_eq!(audio["codecName"], "aac");
        assert_eq!(audio["rtpPayloadType"], 127);
        assert_eq!(audio["timeBase"], "1/48000");
        assert_eq!(audio["channels"], 2);
    }

    #[test]
    fn an_answer_pairs_accepted_streams_with_receiver_ssrcs() {
        let reply = json!({"type": "ANSWER", "seqNum": 3, "result": "ok",
            "answer": {"udpPort": 47439, "sendIndexes": [0, 1], "ssrcs": [12, 13], "castMode": "mirroring"}});
        assert_eq!(
            answer(&reply, 3).unwrap().unwrap(),
            Answer {
                udp_port: 47439,
                accepted: vec![(0, 12), (1, 13)]
            }
        );
        assert!(answer(&reply, 4).is_none(), "an answer to another offer");
        assert!(answer(&json!({"type": "MEDIA_STATUS"}), 3).is_none());
    }

    #[test]
    fn a_refused_offer_reports_the_receivers_reason() {
        let reply = json!({"type": "ANSWER", "seqNum": 1, "result": "error",
            "error": {"code": 2, "description": "no supported streams"}});
        let Some(Err(Error::Rejected { kind, reason })) = answer(&reply, 1) else {
            panic!("expected a rejection");
        };
        assert_eq!(kind, "ANSWER");
        assert_eq!(reason.as_deref(), Some("no supported streams"));
    }
}
