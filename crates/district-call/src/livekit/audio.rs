//! Where a session's sound comes from and where the far end's goes: the
//! desktop's devices, or frames the caller makes and hears itself.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use futures_util::StreamExt;
use livekit::options::TrackPublishOptions;
use livekit::prelude::{
    LocalAudioTrack, LocalTrack, LocalTrackPublication, RemoteAudioTrack, Room, TrackSource,
};
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::AudioSourceOptions;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::{PlatformAudio, RtcAudioSource};

use super::Task;

/// Where a call's sound comes from and where the far end's goes.
#[derive(Clone, Debug)]
pub enum Audio {
    /// The desktop's default microphone and speakers, through WebRTC's own
    /// audio device module (PulseAudio, or PipeWire's PulseAudio server), with
    /// WebRTC's echo cancellation, noise suppression and gain control on the
    /// microphone. This is the app's.
    ///
    /// The devices are opened when a session starts and closed when it ends,
    /// and the microphone is open only while it is on: turning it off mutes
    /// it and stops the capture, so the device is let go at once. When the
    /// devices cannot be opened (no sound server, the desktop refused, or this
    /// process has had a [`Frames`](Self::Frames) microphone), the session
    /// still joins and hears nothing, and the microphone is reported
    /// [`Unavailable`](district_core::MicrophoneState::Unavailable), as it is
    /// when the room refuses it or the device cannot be opened again.
    Devices,
    /// Sound the caller makes and hears itself, with no device opened: the
    /// microphone is whatever [`FrameAudio`]'s microphone writes, and each
    /// remote audio track is handed to its speaker. For tests, and for anything
    /// that is not a desktop. Everything else a session does is the same as
    /// with [`Devices`](Self::Devices), except that its microphone is
    /// [`Unavailable`](district_core::MicrophoneState::Unavailable) in a process
    /// that has opened the devices.
    ///
    /// A process has one or the other for its whole life: whichever of the
    /// devices and a frame microphone it asks for first, it never gets the
    /// other. The media library cannot hold both (see [`claim`]).
    Frames(FrameAudio),
}

/// What a process's microphones are, once it has asked for one: the desktop's
/// devices or the caller's frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Kind {
    Devices = 1,
    Frames = 2,
}

/// The kind this process asked for first, or 0 before it asked for any.
static PROCESS_KIND: AtomicU8 = AtomicU8::new(0);

/// Whether this process may have `kind`: true when it is the first kind the
/// process asks for, or the same one again, and false for good once the
/// process has asked for the other.
///
/// The desktop's devices and a frame microphone never share a process, not
/// only never at once: whichever is asked for second is refused. This is
/// deliberate, because of defects in the libwebrtc the LiveKit SDK links that
/// have been reported privately upstream; this comment will say more once
/// upstream has published a fix. See SECURITY.md.
fn claim(kind: Kind) -> bool {
    claim_in(&PROCESS_KIND, kind)
}

fn claim_in(held: &AtomicU8, kind: Kind) -> bool {
    match held.compare_exchange(0, kind as u8, Ordering::SeqCst, Ordering::SeqCst) {
        Ok(_) => true,
        Err(first) => first == kind as u8,
    }
}

/// Fills one frame the microphone sends.
type MakeFrame = Arc<dyn Fn(&mut [i16]) + Send + Sync>;
/// Takes one frame heard.
type HearFrame = Arc<dyn Fn(&[i16]) + Send + Sync>;

/// The two ends of [`Audio::Frames`]: 48 kHz mono, 16-bit, in frames of
/// 10 ms ([`FrameAudio::SAMPLES`] samples).
#[derive(Clone)]
pub struct FrameAudio {
    microphone: MakeFrame,
    speaker: HearFrame,
}

impl FrameAudio {
    /// The sample rate of every frame, both ways.
    pub const SAMPLE_RATE: u32 = 48_000;
    /// The samples in one frame: 10 ms at [`SAMPLE_RATE`](Self::SAMPLE_RATE).
    pub const SAMPLES: usize = 480;

    /// `microphone` fills each frame the microphone sends, while it is on and
    /// at the pace the media library takes them; `speaker` is handed each frame
    /// heard from each remote audio track, as it is decoded.
    pub fn new(
        microphone: impl Fn(&mut [i16]) + Send + Sync + 'static,
        speaker: impl Fn(&[i16]) + Send + Sync + 'static,
    ) -> Self {
        Self {
            microphone: Arc::new(microphone),
            speaker: Arc::new(speaker),
        }
    }
}

impl fmt::Debug for FrameAudio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameAudio").finish_non_exhaustive()
    }
}

/// Opens the desktop's audio devices for a session, or `None` when they cannot
/// be opened, or may not be because this process has had a frame microphone
/// (see [`claim`]). Off the runtime's threads, because connecting to the sound
/// server blocks.
pub(super) async fn open_devices() -> Option<PlatformAudio> {
    if !claim(Kind::Devices) {
        return None;
    }
    tokio::task::spawn_blocking(PlatformAudio::new)
        .await
        .ok()
        .and_then(Result::ok)
}

/// The microphone, published once in a session and from then on turned off
/// and on by muting it, with the capture stopped while it is off.
///
/// Not unpublished and published again: in a session on the devices, a second
/// microphone track sends nothing but zeros (measured: every run, with or
/// without restarting the capture), and WebRTC stops capturing for an
/// unpublished track only once a renegotiation has turned its sender off,
/// which can lag or stall (measured: one run in three still had the device
/// open 30 seconds later). Muting stops the sending at once; stopping the
/// capture, as the SDK documents for the desktop's recording indicator, lets
/// go of the device at once.
pub(super) struct Microphone {
    publication: LocalTrackPublication,
    capture: Capture,
    on: bool,
}

/// Where the microphone's sound comes from.
enum Capture {
    /// The desktop's microphone, through the session's devices.
    Devices,
    /// The caller's frames: the source they go to, the caller's microphone,
    /// and the task making them while the microphone is on.
    Frames {
        source: NativeAudioSource,
        microphone: MakeFrame,
        making: Option<Task>,
    },
}

impl Microphone {
    /// Publishes the microphone in `room`, on. `None` when there is nothing
    /// to publish from (the devices could not be opened, or cannot capture,
    /// or frames in a process that has opened the devices) or the room
    /// refused it.
    pub(super) async fn publish(
        room: &Room,
        audio: &Audio,
        devices: Option<&PlatformAudio>,
    ) -> Option<Self> {
        let (source, capture) = match audio {
            Audio::Devices => {
                let devices = devices?;
                // A microphone that cannot be opened is unavailable, not a
                // track that sends silence.
                devices.start_recording().ok()?;
                (devices.rtc_source(), Capture::Devices)
            }
            Audio::Frames(frame_audio) => {
                if !claim(Kind::Frames) {
                    return None;
                }
                // 100 ms of queue: `capture_frame` waits while it is full,
                // which is what paces the frames at the rate the library
                // sends them.
                let source = NativeAudioSource::new(
                    AudioSourceOptions::default(),
                    FrameAudio::SAMPLE_RATE,
                    1,
                    100,
                );
                let making = Task::spawn(make_frames(
                    source.clone(),
                    Arc::clone(&frame_audio.microphone),
                ));
                (
                    RtcAudioSource::Native(source.clone()),
                    Capture::Frames {
                        source,
                        microphone: Arc::clone(&frame_audio.microphone),
                        making: Some(making),
                    },
                )
            }
        };
        let track = LocalAudioTrack::create_audio_track("microphone", source);
        let options = TrackPublishOptions {
            source: TrackSource::Microphone,
            ..TrackPublishOptions::default()
        };
        let published = room
            .local_participant()
            .publish_track(LocalTrack::Audio(track), options)
            .await;
        match published {
            Ok(publication) => Some(Self {
                publication,
                capture,
                on: true,
            }),
            Err(_) => {
                if let (Capture::Devices, Some(devices)) = (&capture, devices) {
                    devices.stop_recording().ok();
                }
                None
            }
        }
    }

    /// Whether it is on.
    pub(super) fn is_on(&self) -> bool {
        self.on
    }

    /// Turns it off: muted, so nothing more is sent, and the capture stopped,
    /// so the device is let go. Safe to repeat, which is how a reconnection
    /// that published it again is made to let go of the device again.
    pub(super) fn off(&mut self, devices: Option<&PlatformAudio>) {
        self.publication.mute();
        match &mut self.capture {
            Capture::Devices => {
                if let Some(devices) = devices {
                    devices.stop_recording().ok();
                }
            }
            Capture::Frames { making, .. } => *making = None,
        }
        self.on = false;
    }

    /// Turns it on again: the capture started, then unmuted. False, and still
    /// off, when the device cannot be opened again.
    pub(super) fn turn_on(&mut self, devices: Option<&PlatformAudio>) -> bool {
        match &mut self.capture {
            Capture::Devices => {
                if devices.is_none_or(|devices| devices.start_recording().is_err()) {
                    return false;
                }
            }
            Capture::Frames {
                source,
                microphone,
                making,
            } => {
                *making = Some(Task::spawn(make_frames(
                    source.clone(),
                    Arc::clone(microphone),
                )));
            }
        }
        self.publication.unmute();
        self.on = true;
        true
    }
}

/// Feeds the caller's frames to `source` until the task is stopped or the
/// source is gone.
async fn make_frames(source: NativeAudioSource, microphone: MakeFrame) {
    let mut samples = vec![0; FrameAudio::SAMPLES];
    loop {
        microphone(&mut samples);
        let frame = AudioFrame {
            data: Cow::Borrowed(&samples),
            sample_rate: FrameAudio::SAMPLE_RATE,
            num_channels: 1,
            samples_per_channel: FrameAudio::SAMPLES as u32,
        };
        if source.capture_frame(&frame).await.is_err() {
            break;
        }
    }
}

/// With [`Audio::Frames`], a task handing `track`'s decoded frames to the
/// speaker until it is stopped. With the devices, `None`: WebRTC plays every
/// remote track itself.
pub(super) fn hear(audio: &Audio, track: &RemoteAudioTrack) -> Option<Task> {
    let Audio::Frames(frame_audio) = audio else {
        return None;
    };
    let speaker = Arc::clone(&frame_audio.speaker);
    let mut stream = NativeAudioStream::new(track.rtc_track(), FrameAudio::SAMPLE_RATE as i32, 1);
    Some(Task::spawn(async move {
        while let Some(frame) = stream.next().await {
            speaker(&frame.data);
        }
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU8;

    use super::{Kind, claim_in};

    #[test]
    fn a_process_keeps_the_first_kind_of_microphone_it_asks_for() {
        for (first, other) in [(Kind::Devices, Kind::Frames), (Kind::Frames, Kind::Devices)] {
            let held = AtomicU8::new(0);
            assert!(claim_in(&held, first), "{first:?} first");
            assert!(claim_in(&held, first), "{first:?} again");
            assert!(!claim_in(&held, other), "{other:?} after {first:?}");
            assert!(claim_in(&held, first), "{first:?} after refusing {other:?}");
        }
    }
}
