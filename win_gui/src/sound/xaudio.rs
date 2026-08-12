//! The XAudio2 SFX engine — `gp_audio_win.cpp`'s shape in Rust.
//!
//! The one real structural change from the Mac, in the sister design's words:
//! `AVAudioPlayerNode` sums overlapping buffers internally, but a single
//! `IXAudio2SourceVoice` plays its queue SEQUENTIALLY — so polyphony is a
//! POOL of source voices, and the mastering voice sums them. Same audible
//! result.
//!
//! Inherited decisions, kept deliberately:
//!  * 24 pooled voices, created once at start, alive for the process.
//!  * A busy pool round-robin STEALS the oldest voice (stop + flush) rather
//!    than dropping the new sound — a fresh effect matters more than the
//!    tail of an old one.
//!  * `CoInitializeEx(MULTITHREADED)` unchecked: S_FALSE and a changed
//!    apartment are both fine, and XAudio2.9 on Win10+ barely needs it.
//!  * A failed start (no audio device) leaves everything `None` and every
//!    play a no-op — the game plays on, silent. Init-time only; there is no
//!    device-loss handling, matching the sister.
//!  * Teardown order would be voices → mastering → engine, which is why the
//!    fields sit in that order; in practice the engine lives for the process.

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::XAudio2::{
    IXAudio2, IXAudio2MasteringVoice, IXAudio2SourceVoice, IXAudio2VoiceCallback,
    XAudio2CreateWithVersionInfo, XAUDIO2_BUFFER, XAUDIO2_COMMIT_NOW,
    XAUDIO2_DEFAULT_FREQ_RATIO, XAUDIO2_DEFAULT_PROCESSOR, XAUDIO2_END_OF_STREAM,
    XAUDIO2_VOICE_NOSAMPLESPLAYED, XAUDIO2_VOICE_STATE,
};
use windows::Win32::Media::Audio::{AudioCategory_GameEffects, WAVEFORMATEX};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::SystemInformation::NTDDI_VERSION;

/// Overlapping SFX land on distinct voices; the master sums.
const VOICE_POOL: usize = 24;

/// `WAVE_FORMAT_IEEE_FLOAT`, named locally rather than pulling the whole
/// `Win32_Media_Multimedia` feature in for one integer.
const FORMAT_IEEE_FLOAT: u16 = 3;

/// The one shared format: 44.1 kHz stereo interleaved f32 — the synth's
/// native rate, and (the design notes) SIMPLER than the Mac's conversion:
/// f64 interleaved → f32 interleaved, no deinterleave, because XAudio2 wants
/// interleaved PCM.
fn shared_format() -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: FORMAT_IEEE_FLOAT,
        nChannels: 2,
        nSamplesPerSec: 44_100,
        nAvgBytesPerSec: 44_100 * 8,
        nBlockAlign: 8,
        wBitsPerSample: 32,
        cbSize: 0,
    }
}

// SAFETY: XAudio2 is free-threaded — every interface here is documented
// callable from any thread (the engine runs its own mixing thread), and the
// one `Sfx` lives behind a Mutex touched only by the primary's thread anyway.
// The auto-trait failure is `IXAudio2SourceVoice`'s raw vtable pointer, not a
// real thread affinity.
unsafe impl Send for Sfx {}

pub struct Sfx {
    voices: Vec<IXAudio2SourceVoice>,
    next: usize,
    /// Owned effect buffers, alive until every voice is idle — XAudio2 reads
    /// `pAudioData` IN PLACE for the life of the queue entry, so freeing one
    /// under a playing voice is a use-after-free. Drained opportunistically
    /// at each play, exactly the sister's `retired_` list.
    live: Vec<Box<[f32]>>,
    _master: IXAudio2MasteringVoice,
    _engine: IXAudio2,
}

impl Sfx {
    /// Start the engine, or answer `None` and let the game play on silent.
    pub fn start() -> Option<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let mut engine: Option<IXAudio2> = None;
            XAudio2CreateWithVersionInfo(
                &mut engine,
                0,
                XAUDIO2_DEFAULT_PROCESSOR,
                NTDDI_VERSION,
            )
            .ok()?;
            let engine = engine?;
            let mut master: Option<IXAudio2MasteringVoice> = None;
            engine
                .CreateMasteringVoice(
                    &mut master,
                    2,
                    44_100,
                    0,
                    PCWSTR::null(),
                    None,
                    AudioCategory_GameEffects,
                )
                .ok()?;
            let fmt = shared_format();
            let mut voices = Vec::with_capacity(VOICE_POOL);
            for _ in 0..VOICE_POOL {
                let mut v: Option<IXAudio2SourceVoice> = None;
                if engine
                    .CreateSourceVoice(
                        &mut v,
                        &fmt,
                        0,
                        XAUDIO2_DEFAULT_FREQ_RATIO,
                        None::<&IXAudio2VoiceCallback>,
                        None,
                        None,
                    )
                    .is_ok()
                {
                    if let Some(v) = v {
                        voices.push(v);
                    }
                }
            }
            if voices.is_empty() {
                return None;
            }
            Some(Sfx {
                voices,
                next: 0,
                live: Vec::new(),
                _master: master?,
                _engine: engine,
            })
        }
    }

    fn queued(v: &IXAudio2SourceVoice) -> u32 {
        unsafe {
            let mut st = XAUDIO2_VOICE_STATE::default();
            // NOSAMPLESPLAYED is the cheap form of the query — we only need
            // the queue depth, never the sample position.
            v.GetState(&mut st, XAUDIO2_VOICE_NOSAMPLESPLAYED);
            st.BuffersQueued
        }
    }

    /// A free voice, or the round-robin steal.
    fn pick(&mut self) -> usize {
        for k in 0..self.voices.len() {
            let i = (self.next + k) % self.voices.len();
            if Self::queued(&self.voices[i]) == 0 {
                self.next = (i + 1) % self.voices.len();
                return i;
            }
        }
        let i = self.next % self.voices.len();
        self.next = (i + 1) % self.voices.len();
        unsafe {
            let _ = self.voices[i].Stop(0, XAUDIO2_COMMIT_NOW);
            let _ = self.voices[i].FlushSourceBuffers();
        }
        i
    }

    fn submit(&mut self, ptr: *const f32, samples: usize) {
        let i = self.pick();
        unsafe {
            let buf = XAUDIO2_BUFFER {
                Flags: XAUDIO2_END_OF_STREAM,
                AudioBytes: (samples * 4) as u32,
                pAudioData: ptr as *const u8,
                ..Default::default()
            };
            let v = &self.voices[i];
            if v.SubmitSourceBuffer(&buf, None).is_ok() {
                let _ = v.Start(0, XAUDIO2_COMMIT_NOW);
            }
        }
    }

    /// Play a buffer that lives for the process (a cached preset). No
    /// lifetime question to answer — that is the point of leaking them.
    pub fn play_static(&mut self, pcm: &'static [f32]) {
        self.drain_done();
        if !pcm.is_empty() {
            self.submit(pcm.as_ptr(), pcm.len());
        }
    }

    /// Play an owned buffer (a rendered effect), retiring it to `live` until
    /// every voice reports idle.
    pub fn play_owned(&mut self, pcm: Vec<f32>) {
        self.drain_done();
        if pcm.is_empty() {
            return;
        }
        let b = pcm.into_boxed_slice();
        self.submit(b.as_ptr(), b.len());
        self.live.push(b);
        // The backstop for a pathological editor session that never lets the
        // pool go idle: silence everything and free, rather than grow without
        // bound. Radical, audible, and in practice unreachable — the sister
        // bounds this with 64 fixed slots instead.
        if self.live.len() > 64 {
            for v in &self.voices {
                unsafe {
                    let _ = v.Stop(0, XAUDIO2_COMMIT_NOW);
                    let _ = v.FlushSourceBuffers();
                }
            }
            self.live.clear();
        }
    }

    /// Free retired effect buffers once NO voice is playing — the only
    /// moment a buffer is provably unreferenced without per-buffer contexts.
    fn drain_done(&mut self) {
        if self.live.is_empty() {
            return;
        }
        if self.voices.iter().all(|v| Self::queued(v) == 0) {
            self.live.clear();
        }
    }
}
