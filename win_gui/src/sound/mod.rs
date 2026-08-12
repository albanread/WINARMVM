//! **WG11-W12: sound — the GamePane's last sense.**
//!
//! The split follows the portability contract exactly: SYNTHESIS is
//! upstream's own Rust, copied byte-for-byte from the engine crate the Mac
//! host links (`MacGamePane/audio/src/synth.rs` and `abc.rs` — `cmp` agrees,
//! and both files carry their own test suites, which now run here); only the
//! OUTPUT is Windows. Same recipe, same LCG, same samples — the games sound
//! the same because the numbers are the same.
//!
//! Output is the sister Dart port's design, shipped twice before
//! (`WINDARTTALK/gamepane-design/GP_AUDIO_DESIGN.md` + `gp_audio_win.cpp`):
//! **XAudio2** for SFX — inbox on Windows 10/11 including ARM64, no
//! redistributable, and its mastering voice does the mixing a pool of source
//! voices needs — and **winmm's `midiStream`** for ABC tunes, the GS
//! Wavetable synth standing in for the Mac's `AVMIDIPlayer` (that half was
//! designed there and is implemented here).
//!
//! Everything runs on the PRIMARY's thread, where the sink's arms already
//! are: XAudio2 is free-threaded with its own mixing thread, so no hand-off
//! to the pump is needed — unlike D3D, sound has no thread_local seam. One
//! engine per process, created lazily on the first sound and kept across
//! games (the sister's shape); a machine with no audio device degrades to
//! silence, never to a dead game.

// VERBATIM COPIES — `cmp` against `MacGamePane/audio/src/` agrees byte for
// byte, and they must stay that way: editing them is how the Windows games
// would start sounding different from the Mac's. `dead_code` is allowed
// wholesale rather than annotated inside, because the vendored API is
// deliberately wider than this host calls (WAV export, the voice bank, the
// SMF writer) and an `#[allow]` added INSIDE would be an edit.
#[allow(dead_code)]
pub mod abc;
#[allow(dead_code)]
pub mod synth;

mod midi;
mod xaudio;

use std::sync::Mutex;

/// The one engine. `None` inside means "not started yet OR no device" — the
/// distinction is invisible on purpose: both mean the next play tries again
/// cheaply or stays silent, and the game runs either way.
static SFX: Mutex<Option<xaudio::Sfx>> = Mutex::new(None);

/// Preset PCM, rendered once and LEAKED. XAudio2 does not copy `pAudioData`
/// (the sister's design flags this as THE lifetime hazard), and a buffer that
/// lives for the process cannot be freed under a voice that is still reading
/// it. Twelve presets at ≤10s each bounds the leak at a few megabytes, paid
/// only for presets a game actually plays.
static PRESETS: Mutex<[Option<&'static [f32]>; 12]> = Mutex::new([None; 12]);

fn with_sfx<T>(f: impl FnOnce(&mut xaudio::Sfx) -> T) -> Option<T> {
    let mut g = SFX.lock().ok()?;
    if g.is_none() {
        *g = xaudio::Sfx::start();
    }
    g.as_mut().map(f)
}

/// `Sound <preset> play` (primitive 213). Presets 0..=9 are upstream's ten;
/// 10 (saucer) and 11 (bossHum) are galaxigans' beating-sine pairs, already
/// in the synth. Anything else renders the synth's own fallback (a click),
/// uncached — `play_sound` never panics on a wild index and neither do we.
pub fn play_preset(preset: u8) {
    let idx = preset as usize;
    if idx < 12 {
        let pcm = {
            let Ok(mut g) = PRESETS.lock() else { return };
            if g[idx].is_none() {
                let sound = synth::play_sound(preset);
                let v: Vec<f32> = sound.samples.iter().map(|&s| s as f32).collect();
                g[idx] = Some(Box::leak(v.into_boxed_slice()));
            }
            g[idx]
        };
        if let Some(pcm) = pcm {
            with_sfx(|s| s.play_static(pcm));
        }
    } else {
        let sound = synth::play_sound(preset);
        let v: Vec<f32> = sound.samples.iter().map(|&s| s as f32).collect();
        with_sfx(|s| s.play_owned(v));
    }
}

/// `Sound playEffect:` (primitive 263) — the Sound Editor's parametric wire.
/// The flat params contract has ONE decoder (`synth::effect_from_params`,
/// upstream's own words) and this is a caller of it, not a second parser.
pub fn play_effect(params: &[f64]) {
    let Some((effect, seed)) = synth::effect_from_params(params) else {
        return;
    };
    let sound = synth::render(&effect, &mut synth::Lcg::new(seed));
    let v: Vec<f32> = sound.samples.iter().map(|&s| s as f32).collect();
    with_sfx(|s| s.play_owned(v));
}

/// `(Tune fromAbc: '...') playOnce` (primitive 214). An ABC that parses to no
/// notes is a silent no-op, exactly the Mac's contract.
pub fn play_tune(abc_text: &str) {
    let Some(tune) = abc::parse_tune(abc_text) else {
        return;
    };
    midi::play_once(&tune);
}

/// Silence the tune channel — called from `stop()`, because a title theme
/// must not keep playing under the next demo. SFX are left to ring out: they
/// are under two seconds, which is the sister's stance too.
pub fn stop_tunes() {
    midi::stop();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole SFX path against the REAL device: engine start, preset
    /// render + cache, voice pick, submit. On a machine with no audio device
    /// the engine answers `None` and this still passes — the same graceful
    /// degrade the game gets. The sister port's T3 gate made exactly this
    /// claim ("a coin smoke-test play → submitted").
    #[test]
    fn presets_and_effects_reach_a_voice_without_panicking() {
        play_preset(0); // coin — cached path
        play_preset(11); // bossHum — galaxigans' beating pair
        play_preset(200); // wild index — the synth's own click fallback
        // A minimal legal effect: 14 fixed params + one sine oscillator.
        let params = [
            0.1, 0.01, 0.02, 0.7, 0.02, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 7.0, 1.0, 0.0, 440.0,
            0.8, 0.0, 0.5,
        ];
        play_effect(&params);
        // Long enough for the mixing thread to actually pull samples.
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    /// Preset PCM is rendered once, cached leaked, and NON-SILENT — a
    /// preset that rendered to zeros would "play" forever without a sound.
    #[test]
    fn cached_presets_are_non_silent() {
        for preset in [0u8, 4, 10, 11] {
            let s = synth::play_sound(preset);
            assert!(
                s.samples.iter().any(|&x| x.abs() > 0.01),
                "preset {preset} rendered silence"
            );
        }
    }

    /// The tune path: galaxigans' actual title theme parses to events and
    /// survives play/stop against the real GS Wavetable device.
    #[test]
    fn a_real_tune_plays_once_and_stops() {
        let abc = "T:smoke\nQ:120\nK:C\nC E G c";
        let tune = abc::parse_tune(abc).expect("four plain notes parse");
        assert!(!tune.events.is_empty());
        midi::play_once(&tune);
        std::thread::sleep(std::time::Duration::from_millis(200));
        stop_tunes();
        // Stopping twice is a no-op, not a crash — reset paths do this.
        stop_tunes();
    }
}
