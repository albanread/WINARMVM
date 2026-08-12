//! ABC tunes through winmm's `midiStream` — GP_AUDIO_DESIGN §2, implemented.
//!
//! The Mac plays a `Tune` by writing a temp `.mid` and handing it to
//! `AVMIDIPlayer`, whose General MIDI synth gives the chiptunes real
//! instruments. Windows' twin is the GS Wavetable Synth behind
//! `midiStreamOut` — also inbox, also GM, also ARM64. The sister design
//! worked out the arithmetic and the traps and then shipped a no-op; this is
//! that design, running:
//!
//!  * 480 ticks per quarter, tempo `60_000_000 / bpm`, and the tick for an
//!    absolute-millisecond event is `ms * bpm / 125` — `BuildSmf`'s math.
//!  * The `MIDIHDR` is boxed BEFORE `midiOutPrepareHeader` and never moves
//!    while queued — the driver holds its address.
//!  * `playOnce` only: one stream, one buffer. A new tune replaces the old
//!    (stop → unprepare → close → fresh open), which is also the reset path.
//!    Upstream can overlap two tunes; no game in the corpus does, and
//!    galaxigans' saucer pad UNDER its warble is a tune under an SFX, which
//!    overlaps fine here too.

use windows::core::PSTR;
use windows::Win32::Media::Audio::{
    midiOutPrepareHeader, midiOutUnprepareHeader, midiStreamClose, midiStreamOpen,
    midiStreamOut, midiStreamProperty, midiStreamRestart, midiStreamStop, HMIDIOUT, HMIDISTRM,
    MIDIHDR, MIDIPROPTEMPO, MIDIPROPTIMEDIV, MIDIPROP_SET, MIDIPROP_TEMPO, MIDIPROP_TIMEDIV,
    MEVT_F_SHORT,
};

use super::abc::Tune;
use std::sync::Mutex;

/// 480 ticks per quarter note — `BuildSmf`'s division, kept so the tick math
/// below is exactly its `tick = ms * bpm / 125`.
const PPQ: u32 = 480;

struct Stream {
    h: HMIDISTRM,
    /// Boxed so its address is stable from prepare to unprepare.
    hdr: Box<MIDIHDR>,
    /// The event words the header points into; alive as long as it is.
    _data: Vec<u32>,
}

// SAFETY: the handle and header are only ever touched under the STREAM lock,
// and winmm's stream API is callable from any thread.
unsafe impl Send for Stream {}

static STREAM: Mutex<Option<Stream>> = Mutex::new(None);

/// Stop and tear down the current tune, if any. Note-offs are the driver's
/// job on `midiStreamStop`, so nothing hangs.
pub fn stop() {
    let Ok(mut g) = STREAM.lock() else { return };
    if let Some(mut s) = g.take() {
        unsafe {
            let _ = midiStreamStop(s.h);
            let _ = midiOutUnprepareHeader(
                HMIDIOUT(s.h.0),
                &mut *s.hdr,
                std::mem::size_of::<MIDIHDR>() as u32,
            );
            let _ = midiStreamClose(s.h);
        }
    }
}

/// Play `tune` once in the background, replacing any tune already playing.
pub fn play_once(tune: &Tune) {
    if tune.events.is_empty() {
        return;
    }
    stop();

    // Absolute milliseconds → stream words: [delta ticks, stream id, event].
    // MEVT_SHORTMSG is 0, so a short event's word is just the packed message.
    let bpm = tune.bpm.max(1);
    let mut data: Vec<u32> = Vec::with_capacity(tune.events.len() * 3);
    let mut last_tick: u64 = 0;
    for e in &tune.events {
        let tick = e.time_ms as u64 * bpm as u64 / 125;
        let delta = tick.saturating_sub(last_tick);
        last_tick = tick;
        let msg = (e.status | e.chan) as u32 | (e.d1 as u32) << 8 | (e.d2 as u32) << 16;
        data.push(delta as u32);
        data.push(0);
        data.push(MEVT_F_SHORT as u32 | msg);
    }

    unsafe {
        let mut h = HMIDISTRM::default();
        // Device u32::MAX is MIDI_MAPPER — the default GM synth.
        let mut device = [u32::MAX];
        if midiStreamOpen(&mut h, &mut device, None, None, 0) != 0 {
            return;
        }
        let mut div = MIDIPROPTIMEDIV {
            cbStruct: std::mem::size_of::<MIDIPROPTIMEDIV>() as u32,
            dwTimeDiv: PPQ,
        };
        let _ = midiStreamProperty(
            h,
            &mut div as *mut _ as *mut u8,
            (MIDIPROP_SET | MIDIPROP_TIMEDIV) as u32,
        );
        let mut tempo = MIDIPROPTEMPO {
            cbStruct: std::mem::size_of::<MIDIPROPTEMPO>() as u32,
            dwTempo: 60_000_000 / bpm,
        };
        let _ = midiStreamProperty(
            h,
            &mut tempo as *mut _ as *mut u8,
            (MIDIPROP_SET | MIDIPROP_TEMPO) as u32,
        );

        let mut hdr = Box::new(MIDIHDR {
            lpData: PSTR(data.as_ptr() as *mut u8),
            dwBufferLength: (data.len() * 4) as u32,
            dwBytesRecorded: (data.len() * 4) as u32,
            ..Default::default()
        });
        let cb = std::mem::size_of::<MIDIHDR>() as u32;
        if midiOutPrepareHeader(HMIDIOUT(h.0), &mut *hdr, cb) != 0
            || midiStreamOut(h, &mut *hdr, cb) != 0
            || midiStreamRestart(h) != 0
        {
            let _ = midiOutUnprepareHeader(HMIDIOUT(h.0), &mut *hdr, cb);
            let _ = midiStreamClose(h);
            return;
        }
        if let Ok(mut g) = STREAM.lock() {
            *g = Some(Stream {
                h,
                hdr,
                _data: data,
            });
        }
    }
}
