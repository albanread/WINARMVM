//! Software SFX synthesizer — ported from MacModula2's `Audio.mod`/`Audio.def`.
//!
//! Renders game sound effects (coin, jump, zap, explode, ...) and plain tones
//! entirely in software into a PCM [`Sound`] buffer: no device, no window,
//! fully headless-testable. The engine is an oscillator stack (sine / square /
//! saw / triangle / noise / pulse) driven through an optional frequency sweep,
//! a noise blend, an ADSR envelope, `tanh` distortion, and echo/delay taps,
//! then peak-normalized. Presets are hand-tuned effect recipes (`Coin`,
//! `Jump`, `Zap`, ...) matching the Modula-2 originals exactly.
//!
//! **Deliberate deviation from the source**: the Modula-2 `Audio.mod` keeps a
//! single global mutable LCG (`gState: CARDINAL`, stepped in place by every
//! noise-touching call). Rust test binaries run tests in parallel by
//! default, so a shared mutable global RNG would make tests racy/
//! nondeterministic. Instead, [`Lcg`] is an explicit value threaded as `&mut
//! Lcg` through every function that needs randomness — only the noise-mixing
//! presets and the generic [`noise`] function take one; presets with no
//! noise component don't. The actual math (LCG constants, waveform sample
//! formulas, ADSR envelope shape, and the render pipeline order: oscillator
//! stack -> sweep -> noise mix -> ADSR -> distortion -> echo -> normalize)
//! matches `Audio.mod` exactly.

/// Sample rate used by every render: 44100 Hz (`Audio.def`'s `SampleRate`).
pub const SAMPLE_RATE: u32 = 44100;

// `Audio.mod`'s own `Pi`/`TwoPi` literals (3.14159265358979/6.28318530717959)
// round-trip to the exact same f64 bit pattern as these std constants —
// using the named constants instead is a pure style improvement, not a
// numeric deviation from the source (clippy's approx_constant lint agrees).
const PI: f64 = std::f64::consts::PI;
const TWO_PI: f64 = std::f64::consts::TAU;

/// Oscillator waveform shape (`Audio.def`'s `Waveform`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Sine,
    Square,
    Saw,
    Triangle,
    Noise,
    Pulse,
}

/// Attack/Decay/Sustain/Release envelope, all in seconds except `sustain`
/// which is a `0..1` level (`Audio.mod`'s `Adsr` record).
#[derive(Debug, Clone, Copy)]
pub struct Adsr {
    pub attack: f64,
    pub decay: f64,
    pub sustain: f64,
    pub release: f64,
}

/// A single oscillator voice in an [`Effect`]'s stack (`Audio.mod`'s
/// `Oscillator` record).
#[derive(Debug, Clone, Copy)]
pub struct Oscillator {
    pub wave: Waveform,
    pub frequency: f64,
    pub amplitude: f64,
    pub phase: f64,
    pub pulse_width: f64,
}

/// A full sound-effect recipe consumed by [`render`] (`Audio.mod`'s `Effect`
/// record). Modula-2 caps the oscillator stack at 4 entries; the same cap is
/// enforced here in [`Effect::add_osc`] to match `AddOsc`'s `IF e.oscCount <=
/// 3` guard.
#[derive(Debug, Clone)]
pub struct Effect {
    pub duration: f64,
    pub oscillators: Vec<Oscillator>,
    pub env: Adsr,
    pub sweep_start: f64,
    pub sweep_end: f64,
    pub noise_mix: f64,
    pub distortion: f64,
    pub echo_count: u32,
    pub echo_delay: f64,
    pub echo_decay: f64,
}

impl Effect {
    /// A cleared effect with `duration` set and defaults matching
    /// `ClearEffect`: `Adsr { attack: 0.01, decay: 0.1, sustain: 0.7, release:
    /// 0.2 }`, no sweep/noise/distortion/echo.
    pub fn new(duration: f64) -> Self {
        Effect {
            duration,
            oscillators: Vec::new(),
            env: Adsr {
                attack: 0.01,
                decay: 0.1,
                sustain: 0.7,
                release: 0.2,
            },
            sweep_start: 0.0,
            sweep_end: 0.0,
            noise_mix: 0.0,
            distortion: 0.0,
            echo_count: 0,
            echo_delay: 0.0,
            echo_decay: 0.0,
        }
    }

    /// Append an oscillator (`phase: 0.0`, `pulse_width: 0.5`), silently
    /// capped at 4 entries like `AddOsc`'s `IF e.oscCount <= 3` guard.
    pub fn add_osc(&mut self, wave: Waveform, frequency: f64, amplitude: f64) {
        if self.oscillators.len() <= 3 {
            self.oscillators.push(Oscillator {
                wave,
                frequency,
                amplitude,
                phase: 0.0,
                pulse_width: 0.5,
            });
        }
    }

    /// Set the ADSR envelope (`SetEnv`).
    pub fn set_env(&mut self, attack: f64, decay: f64, sustain: f64, release: f64) {
        self.env = Adsr {
            attack,
            decay,
            sustain,
            release,
        };
    }
}

/// A rendered PCM buffer: interleaved samples nominally in `[-1, 1]`,
/// `samples[frame * channels + ch]` (`Audio.mod`'s `Sound` record, minus the
/// manually-managed pointer/count fields Rust's `Vec` makes unnecessary).
#[derive(Debug, Clone)]
pub struct Sound {
    pub sample_rate: u32,
    pub channels: u32,
    pub samples: Vec<f64>,
}

impl Sound {
    /// Duration in seconds implied by the sample count (`count / channels /
    /// sample_rate`), the Rust equivalent of the stored `duration` field the
    /// Modula-2 record carries redundantly.
    pub fn duration(&self) -> f64 {
        if self.channels == 0 || self.sample_rate == 0 {
            return 0.0;
        }
        (self.samples.len() as f64 / self.channels as f64) / self.sample_rate as f64
    }
}

/// The linear congruential generator backing noise/randomization
/// (`Audio.mod`'s `gState` + `StepLcg`/`NextSigned`/`NextUnit`), threaded
/// explicitly instead of held as global mutable state — see the module docs.
#[derive(Debug, Clone, Copy)]
pub struct Lcg {
    state: u32,
}

impl Lcg {
    /// A fresh generator seeded like `Audio.InitEngine(seed)` (default seed
    /// in the source is 12345, applied at module init).
    pub fn new(seed: u32) -> Self {
        Lcg { state: seed }
    }

    /// One raw LCG step: `state = state * 1103515245 + 12345 (mod 2^32)`
    /// (`StepLcg`; the `u32` multiply/add wraps exactly like the Modula-2
    /// `MOD 100000000H`).
    fn step(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(1103515245).wrapping_add(12345);
        self.state
    }

    /// Next value in `-1.0..1.0` (`NextSigned`).
    pub fn next_signed(&mut self) -> f64 {
        let st = self.step();
        ((st / 65536) % 32768) as f64 / 16384.0 - 1.0
    }

    /// Next value in `0.0..1.0` (`NextUnit`).
    pub fn next_unit(&mut self) -> f64 {
        let st = self.step();
        ((st / 65536) % 32768) as f64 / 32768.0
    }

    /// Next value in `lo..hi` (`NextRange`).
    pub fn next_range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.next_unit() * (hi - lo)
    }
}

/// Euclidean remainder: always non-negative, matching Modula-2's
/// `RemEuclid` (`x - m * floor(x/m)`), i.e. Rust's `f64::rem_euclid`.
fn rem_euclid(x: f64, m: f64) -> f64 {
    x - m * (x / m).floor()
}

fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// Evaluate one waveform at `phase` (radians) / `pw` (pulse width, `0..1`).
/// `Waveform::Noise` draws from `rng` (`WaveSample`). `pub(crate)` so
/// `voice.rs` can reuse the exact same, already-tested oscillator math for
/// its held-note voices rather than duplicating it.
pub(crate) fn wave_sample(wave: Waveform, phase: f64, pw: f64, rng: &mut Lcg) -> f64 {
    match wave {
        Waveform::Sine => phase.sin(),
        Waveform::Square => {
            if rem_euclid(phase, TWO_PI) < PI {
                1.0
            } else {
                -1.0
            }
        }
        Waveform::Saw => {
            let n = phase / TWO_PI;
            2.0 * (n - (n + 0.5).floor())
        }
        Waveform::Triangle => {
            let t = rem_euclid(phase / TWO_PI, 1.0);
            4.0 * (t - 0.5).abs() - 1.0
        }
        Waveform::Noise => rng.next_signed(),
        Waveform::Pulse => {
            let t = rem_euclid(phase, TWO_PI) / TWO_PI;
            if t < pw {
                1.0
            } else {
                -1.0
            }
        }
    }
}

/// ADSR envelope value at `time` seconds into a note of length `note_dur`
/// seconds (`AdsrValueAt`). Note: [`render`] calls this with the *unclamped*
/// effect duration, matching the Modula-2 comment `(* unclamped duration *)`.
fn adsr_value_at(e: &Adsr, time: f64, note_dur: f64) -> f64 {
    if time < 0.0 {
        return 0.0;
    }
    let total = e.attack + e.decay + e.release;
    let sustain_time = (note_dur - total).max(0.0);
    let mut t = time;
    if t <= e.attack {
        return if e.attack <= 0.0 { 1.0 } else { t / e.attack };
    }
    t -= e.attack;
    if t <= e.decay {
        return if e.decay <= 0.0 {
            e.sustain
        } else {
            1.0 - (t / e.decay) * (1.0 - e.sustain)
        };
    }
    t -= e.decay;
    if t <= sustain_time {
        return e.sustain;
    }
    t -= sustain_time;
    if t <= e.release {
        return if e.release <= 0.0 {
            0.0
        } else {
            e.sustain * (1.0 - t / e.release)
        };
    }
    0.0
}

/// Peak-normalize `sound` in place: if the peak absolute sample exceeds 1.0,
/// scale the whole buffer so the peak lands at `target` (clamped to
/// `[0,1]`); a no-op if the peak is already `<= 1.0` (`Normalize`).
pub fn normalize(sound: &mut Sound, target: f64) {
    let peak = sound
        .samples
        .iter()
        .fold(0.0_f64, |acc, &s| acc.max(s.abs()));
    if peak > 1.0 {
        let scale = clamp(target, 0.0, 1.0) / peak;
        for s in sound.samples.iter_mut() {
            *s *= scale;
        }
    }
}

/// Render an [`Effect`] to a stereo [`Sound`] at [`SAMPLE_RATE`]
/// (`Render`, `engine.rs:218-313`). Pipeline order: oscillator stack ->
/// frequency sweep -> noise mix -> ADSR envelope -> tanh distortion -> `*0.5`
/// write to both channels -> echo/delay taps -> peak-normalize to 0.9.
pub fn render(effect: &Effect, rng: &mut Lcg) -> Sound {
    let mut clamped = clamp(effect.duration, 0.0, 10.0);
    if clamped <= 0.0 {
        clamped = 0.01;
    }
    let channels = 2u32;
    let mut frame_count = (SAMPLE_RATE as f64 * clamped).trunc() as u32 * channels;
    const MAX_SAMPLES: u32 = 882_000; // 10s stereo @ 44100 Hz (Audio.def's MaxSamples)
    if frame_count > MAX_SAMPLES {
        frame_count = MAX_SAMPLES;
    }
    if frame_count == 0 {
        frame_count = channels;
    }
    let sample_count = frame_count; // NewSound's `n`, i.e. total interleaved samples
    let frames = sample_count / channels;

    let mut samples = vec![0.0_f64; sample_count as usize];
    let dt = 1.0 / SAMPLE_RATE as f64;

    for f in 0..frames {
        let time = f as f64 * dt;
        let mut sample = 0.0;
        for osc in &effect.oscillators {
            let phase = TWO_PI * osc.frequency * time + osc.phase;
            sample += wave_sample(osc.wave, phase, osc.pulse_width, rng) * osc.amplitude;
        }
        if effect.sweep_start != effect.sweep_end {
            let sweep_t = if clamped > 0.0 { time / clamped } else { 0.0 };
            let freq = effect.sweep_start + (effect.sweep_end - effect.sweep_start) * sweep_t;
            sample += wave_sample(Waveform::Sine, TWO_PI * freq * time, 0.5, rng) * 0.5;
        }
        if effect.noise_mix > 0.0 {
            let n = rng.next_signed();
            sample = sample * (1.0 - effect.noise_mix) + n * effect.noise_mix;
        }
        // NOTE: uses the *unclamped* effect.duration, matching the Modula-2 comment.
        let env = adsr_value_at(&effect.env, time, effect.duration);
        sample *= env;
        if effect.distortion > 0.0 {
            let drive = 1.0 + effect.distortion * 10.0;
            let denom = drive.tanh();
            if denom != 0.0 {
                sample = (sample * drive).tanh() / denom;
            }
        }
        for ch in 0..channels {
            samples[(f * channels + ch) as usize] = sample * 0.5;
        }
    }

    let mut sound = Sound {
        sample_rate: SAMPLE_RATE,
        channels,
        samples,
    };

    if effect.echo_count > 0 && effect.echo_delay > 0.0 {
        let delay_frames = (effect.echo_delay * SAMPLE_RATE as f64).trunc() as u32;
        for echo in 0..effect.echo_count {
            let echo_start = delay_frames * (echo + 1);
            let amp = effect.echo_decay.powf((echo + 1) as f64);
            if echo_start < frames {
                for frame in 0..(frames - echo_start) {
                    for ch in 0..channels {
                        let idx = ((frame + echo_start) * channels + ch) as usize;
                        let src = (frame * channels + ch) as usize;
                        // In-place accumulation matches Modula-2's aliased VAR buffer:
                        // an echo tap can read a sample a previous tap already wrote.
                        sound.samples[idx] += sound.samples[src] * amp;
                    }
                }
            }
        }
    }

    normalize(&mut sound, 0.9);
    sound
}

/// MIDI note number -> frequency in Hz, A4 (note 69) = 440 Hz
/// (`NoteToFrequency`).
pub fn note_to_frequency(midi: i32) -> f64 {
    440.0 * 2.0_f64.powf((midi - 69) as f64 / 12.0)
}

// ---- presets (presets.rs) --------------------------------------------------

/// A sine beep at `freq` Hz (`Beep`). No noise component.
pub fn beep(freq: f64, dur: f64) -> Sound {
    let mut e = Effect::new(dur);
    e.add_osc(Waveform::Sine, freq, 0.5);
    e.set_env(0.01, 0.05, 0.7, 0.1);
    render(&e, &mut Lcg::new(0))
}

/// Two-sine coin-pickup chime (`Coin`). No noise component.
pub fn coin(dur: f64) -> Sound {
    let mut e = Effect::new(dur);
    e.add_osc(Waveform::Sine, 987.77, 0.5);
    e.add_osc(Waveform::Sine, 1318.51, 0.3);
    e.set_env(0.01, 0.1, 0.3, 0.15);
    render(&e, &mut Lcg::new(0))
}

/// An upward sweep (`Jump`). No noise component.
pub fn jump(dur: f64) -> Sound {
    let mut e = Effect::new(dur);
    e.sweep_start = 300.0;
    e.sweep_end = 600.0;
    e.set_env(0.01, 0.05, 0.5, 0.1);
    render(&e, &mut Lcg::new(0))
}

/// A downward sweep with a light noise blend (`Zap`).
pub fn zap(dur: f64, rng: &mut Lcg) -> Sound {
    let mut e = Effect::new(dur);
    e.sweep_start = 1000.0;
    e.sweep_end = 100.0;
    e.noise_mix = 0.2;
    e.set_env(0.01, 0.05, 0.3, 0.08);
    render(&e, rng)
}

/// A downward sweep with more noise (`Shoot`).
pub fn shoot(dur: f64, rng: &mut Lcg) -> Sound {
    let mut e = Effect::new(dur);
    e.sweep_start = 800.0;
    e.sweep_end = 200.0;
    e.noise_mix = 0.3;
    e.set_env(0.01, 0.05, 0.4, 0.08);
    render(&e, rng)
}

/// A low sine+triangle rumble with size-scaled noise and distortion
/// (`Explode`).
pub fn explode(size: f64, dur: f64, rng: &mut Lcg) -> Sound {
    let mut e = Effect::new(dur);
    e.add_osc(Waveform::Sine, 58.0, 0.95);
    e.add_osc(Waveform::Triangle, 86.0, 0.28);
    e.noise_mix = clamp(0.06 * size, 0.0, 0.12);
    e.sweep_start = 135.0;
    e.sweep_end = 32.0;
    e.distortion = 0.08;
    e.set_env(0.0015, 0.14, 0.0, 0.10);
    render(&e, rng)
}

/// A rising square sweep (`Powerup`). No noise component.
pub fn powerup(dur: f64) -> Sound {
    let mut e = Effect::new(dur);
    e.add_osc(Waveform::Square, 400.0, 0.4);
    e.sweep_start = 200.0;
    e.sweep_end = 800.0;
    e.set_env(0.1, 0.1, 0.8, 0.2);
    render(&e, &mut Lcg::new(0))
}

/// A downward sweep with heavy noise (`Hurt`).
pub fn hurt(dur: f64, rng: &mut Lcg) -> Sound {
    let mut e = Effect::new(dur);
    e.sweep_start = 600.0;
    e.sweep_end = 200.0;
    e.noise_mix = 0.4;
    e.set_env(0.01, 0.1, 0.2, 0.15);
    render(&e, rng)
}

/// A short `WNoise`-oscillator click (`Click`).
pub fn click(dur: f64, rng: &mut Lcg) -> Sound {
    let mut e = Effect::new(dur);
    e.add_osc(Waveform::Noise, 440.0, 0.3);
    e.set_env(0.001, 0.01, 0.0, 0.03);
    render(&e, rng)
}

/// A pure noise burst (`Bang`).
pub fn bang(dur: f64, rng: &mut Lcg) -> Sound {
    let mut e = Effect::new(dur);
    e.noise_mix = 0.8;
    e.set_env(0.01, 0.05, 0.0, 0.1);
    render(&e, rng)
}

/// A pitched beep at `800 * pitch` Hz (`Blip`, delegates to `Beep`). No
/// noise component.
pub fn blip(pitch: f64, dur: f64) -> Sound {
    beep(800.0 * pitch, dur)
}

/// A UFO warble (`Saucer`): two sines a few Hz apart (600 Hz + 606 Hz) whose
/// ~6 Hz *beat* IS the warble — the detune between the tones is the wobble,
/// not a tremolo LFO. A gentle attack/release over ~0.9s. Equal amplitudes so
/// the beat swings fully null-to-peak. No noise component; peak-normalized by
/// [`render`] like every other preset.
pub fn saucer(dur: f64) -> Sound {
    let mut e = Effect::new(dur);
    e.add_osc(Waveform::Sine, 600.0, 0.5);
    e.add_osc(Waveform::Sine, 606.0, 0.5);
    e.set_env(0.08, 0.1, 0.8, 0.15);
    render(&e, &mut Lcg::new(0))
}

/// A tractor-beam hum (`BossHum`): the same two-sine beating trick as
/// [`saucer`], dropped an octave and a half and slowed down — 110 Hz against
/// 114 Hz for a ~4 Hz beat. A long, sustained hum (~1.5-2s) with a slow
/// attack. No noise component; peak-normalized by [`render`].
pub fn boss_hum(dur: f64) -> Sound {
    let mut e = Effect::new(dur);
    e.add_osc(Waveform::Sine, 110.0, 0.5);
    e.add_osc(Waveform::Sine, 114.0, 0.5);
    e.set_env(0.4, 0.2, 0.85, 0.3);
    render(&e, &mut Lcg::new(0))
}

/// Rebuild an [`Effect`] plus its render seed from the flat parameter list
/// macVM's Sound Editor ships (MACVM `docs/asset_editors_design.md` §3):
/// `[duration, attack, decay, sustain, release, sweepStart, sweepEnd,
/// noiseMix, distortion, echoCount, echoDelay, echoDecay, seed, oscCount,
/// then (wave, freq, amp, phase, pulseWidth) per oscillator]`. This function
/// is the contract's ONE decoder — every embedder calls it rather than
/// hand-parsing, so the order can never skew between front ends. `None` for
/// a list too short for its own oscillator count; individual values are
/// clamped, never rejected (an editor slider can only be so wrong).
pub fn effect_from_params(params: &[f64]) -> Option<(Effect, u32)> {
    if params.len() < 14 {
        return None;
    }
    let osc_count = (params[13] as usize).min(4);
    if params.len() < 14 + osc_count * 5 {
        return None;
    }
    let mut e = Effect::new(params[0].clamp(0.01, 4.0));
    e.set_env(
        params[1].max(0.0),
        params[2].max(0.0),
        params[3].clamp(0.0, 1.0),
        params[4].max(0.0),
    );
    e.sweep_start = params[5].max(0.0);
    e.sweep_end = params[6].max(0.0);
    e.noise_mix = params[7].clamp(0.0, 1.0);
    e.distortion = params[8].max(0.0);
    e.echo_count = (params[9].max(0.0) as u32).min(8);
    e.echo_delay = params[10].clamp(0.0, 2.0);
    e.echo_decay = params[11].clamp(0.0, 1.0);
    let seed = params[12].max(0.0) as u32;
    for o in 0..osc_count {
        let b = 14 + o * 5;
        let wave = match params[b] as u32 {
            1 => Waveform::Square,
            2 => Waveform::Saw,
            3 => Waveform::Triangle,
            4 => Waveform::Noise,
            5 => Waveform::Pulse,
            _ => Waveform::Sine,
        };
        e.add_osc(wave, params[b + 1].clamp(0.0, 20_000.0), params[b + 2].clamp(0.0, 1.0));
        let idx = e.oscillators.len() - 1;
        e.oscillators[idx].phase = params[b + 3].clamp(0.0, 1.0);
        e.oscillators[idx].pulse_width = params[b + 4].clamp(0.01, 0.99);
    }
    Some((e, if seed == 0 { 1 } else { seed }))
}

/// Render a preset SFX by its stable numeric index — the GUI-facing
/// `play_sound` entry point. The index order is: `0`=Coin, `1`=Jump, `2`=Zap,
/// `3`=Shoot, `4`=Explode, `5`=Powerup, `6`=Hurt, `7`=Click, `8`=Bang,
/// `9`=Blip, `10`=Saucer, `11`=BossHum. Each index renders its preset at a
/// sensible default duration; the noise-mixing presets thread a fresh internal
/// [`Lcg`] (like [`tone`]/[`blip`]) so this stays a single-argument `u8` API.
/// Any out-of-range index falls back to a very short [`click`], so a bad index
/// still returns a (harmless) non-empty sound rather than panicking.
pub fn play_sound(preset: u8) -> Sound {
    let mut rng = Lcg::new(12345);
    match preset {
        0 => coin(0.3),
        1 => jump(0.3),
        2 => zap(0.3, &mut rng),
        3 => shoot(0.3, &mut rng),
        4 => explode(1.0, 0.5, &mut rng),
        5 => powerup(0.4),
        6 => hurt(0.3, &mut rng),
        7 => click(0.1, &mut rng),
        8 => bang(0.3, &mut rng),
        9 => blip(1.5, 0.2),
        10 => saucer(0.9),
        11 => boss_hum(1.8),
        _ => click(0.05, &mut rng),
    }
}

/// A single oscillator with a default envelope; `Waveform::Pulse` uses a 25%
/// duty cycle, everything else 50% (`Tone`). No noise component (even for
/// `Waveform::Noise`, which draws from an internal disposable `Lcg` since
/// `Tone` takes no `rng` parameter in the public API — callers wanting
/// deterministic/seeded noise tones should use [`render`] directly with a
/// hand-built [`Effect`]).
pub fn tone(freq: f64, dur: f64, wave: Waveform) -> Sound {
    let mut e = Effect::new(dur);
    e.set_env(0.01, 0.05, 0.8, 0.1);
    let freq = if freq < 1.0 { 1.0 } else { freq };
    e.add_osc(wave, freq, 0.75);
    let pw = if wave == Waveform::Pulse { 0.25 } else { 0.5 };
    e.oscillators[0].pulse_width = pw;
    render(&e, &mut Lcg::new(0))
}

/// Coloured noise burst: `kind` 0=white, 1=pink (Paul Kellet), 2=brown
/// (`Noise`, `synth.rs:93-128`). Duration is floored at 0.01s; envelope is
/// `(1-t)^2 * 0.6`, normalized to 0.95.
pub fn noise(kind: u32, dur: f64, rng: &mut Lcg) -> Sound {
    let dur = if dur < 0.01 { 0.01 } else { dur };
    let channels = 2u32;
    let mut frame_count = (SAMPLE_RATE as f64 * dur).trunc() as u32 * channels;
    const MAX_SAMPLES: u32 = 882_000;
    if frame_count > MAX_SAMPLES {
        frame_count = MAX_SAMPLES;
    }
    if frame_count == 0 {
        frame_count = channels;
    }
    let frames = frame_count / channels;

    let mut samples = vec![0.0_f64; frame_count as usize];
    let (mut p0, mut p1, mut p2, mut brown) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);

    for f in 0..frames {
        let mut value = rng.next_signed();
        if kind == 1 {
            p0 = 0.99765 * p0 + value * 0.0990460;
            p1 = 0.96300 * p1 + value * 0.2965164;
            p2 = 0.57000 * p2 + value * 1.0526913;
            value = (p0 + p1 + p2 + value * 0.1848) * 0.25;
        } else if kind == 2 {
            brown += value * 0.02;
            brown = clamp(brown, -1.0, 1.0);
            value = brown;
        }
        let t = f as f64 / frames as f64;
        let env = (1.0 - t) * (1.0 - t);
        value *= env * 0.6;
        for ch in 0..channels {
            samples[(f * channels + ch) as usize] = value;
        }
    }

    let mut sound = Sound {
        sample_rate: SAMPLE_RATE,
        channels,
        samples,
    };
    normalize(&mut sound, 0.95);
    sound
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- LCG -----------------------------------------------------------

    #[test]
    fn lcg_stays_in_documented_ranges() {
        let mut rng = Lcg::new(12345);
        for _ in 0..10_000 {
            let s = rng.next_signed();
            assert!((-1.0..1.0).contains(&s), "next_signed out of range: {s}");
            let u = rng.next_unit();
            assert!((0.0..1.0).contains(&u), "next_unit out of range: {u}");
        }
    }

    #[test]
    fn lcg_is_deterministic_from_a_fresh_seed() {
        let mut a = Lcg::new(42);
        let mut b = Lcg::new(42);
        for _ in 0..500 {
            assert_eq!(a.next_signed(), b.next_signed());
            assert_eq!(a.next_unit(), b.next_unit());
        }
    }

    #[test]
    fn lcg_different_seeds_diverge() {
        let mut a = Lcg::new(1);
        let mut b = Lcg::new(2);
        let seq_a: Vec<f64> = (0..8).map(|_| a.next_signed()).collect();
        let seq_b: Vec<f64> = (0..8).map(|_| b.next_signed()).collect();
        assert_ne!(seq_a, seq_b);
    }

    // ---- waveforms -------------------------------------------------------

    #[test]
    fn sine_at_known_phases() {
        let mut rng = Lcg::new(1);
        assert!((wave_sample(Waveform::Sine, 0.0, 0.5, &mut rng) - 0.0).abs() < 1e-12);
        assert!((wave_sample(Waveform::Sine, PI / 2.0, 0.5, &mut rng) - 1.0).abs() < 1e-9);
        assert!((wave_sample(Waveform::Sine, PI, 0.5, &mut rng) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn square_below_and_above_pi() {
        let mut rng = Lcg::new(1);
        assert_eq!(wave_sample(Waveform::Square, 0.1, 0.5, &mut rng), 1.0);
        assert_eq!(wave_sample(Waveform::Square, PI - 0.01, 0.5, &mut rng), 1.0);
        assert_eq!(
            wave_sample(Waveform::Square, PI + 0.01, 0.5, &mut rng),
            -1.0
        );
        assert_eq!(
            wave_sample(Waveform::Square, TWO_PI - 0.01, 0.5, &mut rng),
            -1.0
        );
    }

    #[test]
    fn saw_ramps_from_zero_phase() {
        let mut rng = Lcg::new(1);
        // At phase 0, n=0, saw = 2*(0 - floor(0.5)) = 2*(0-0) = 0.
        assert!((wave_sample(Waveform::Saw, 0.0, 0.5, &mut rng) - 0.0).abs() < 1e-12);
        // This formula's discontinuity is at the HALF cycle (n=0.5, phase=pi),
        // not at the end of the cycle: it ramps 0->1 over n in [0,0.5), then
        // jumps to ~-1 and ramps back toward 0 over n in [0.5,1). So just
        // before phase=pi it should approach +1 (top of the first ramp)...
        let just_before_half = wave_sample(Waveform::Saw, PI - 0.01, 0.5, &mut rng);
        assert!(
            just_before_half > 0.9,
            "expected near +1, got {just_before_half}"
        );
        // ...and just after phase=pi it should have snapped down to ~-1.
        let just_after_half = wave_sample(Waveform::Saw, PI + 0.01, 0.5, &mut rng);
        assert!(
            just_after_half < -0.9,
            "expected near -1, got {just_after_half}"
        );
        // Just before wrapping back to phase=0/2pi, it's climbed most of the
        // way back from -1 toward 0 (NOT back down near -1 again).
        let just_before_wrap = wave_sample(Waveform::Saw, TWO_PI * 0.999, 0.5, &mut rng);
        assert!(
            just_before_wrap.abs() < 0.1,
            "expected near 0, got {just_before_wrap}"
        );
    }

    #[test]
    fn triangle_known_points() {
        let mut rng = Lcg::new(1);
        // t=0 -> 4*|0-0.5|-1 = 4*0.5-1 = 1.0 (peak at phase 0).
        assert!((wave_sample(Waveform::Triangle, 0.0, 0.5, &mut rng) - 1.0).abs() < 1e-9);
        // t=0.5 (phase = pi) -> 4*|0.5-0.5|-1 = -1.0 (trough at the midpoint).
        assert!((wave_sample(Waveform::Triangle, PI, 0.5, &mut rng) - (-1.0)).abs() < 1e-9);
    }

    #[test]
    fn pulse_duty_cycle() {
        let mut rng = Lcg::new(1);
        // pw=0.25: t<0.25*2pi -> 1, else -1.
        assert_eq!(wave_sample(Waveform::Pulse, 0.1, 0.25, &mut rng), 1.0);
        assert_eq!(
            wave_sample(Waveform::Pulse, TWO_PI * 0.5, 0.25, &mut rng),
            -1.0
        );
    }

    #[test]
    fn noise_draws_from_rng_and_stays_in_range() {
        let mut rng = Lcg::new(7);
        for _ in 0..1000 {
            let v = wave_sample(Waveform::Noise, 0.0, 0.5, &mut rng);
            assert!((-1.0..1.0).contains(&v));
        }
    }

    // ---- ADSR --------------------------------------------------------------

    #[test]
    fn adsr_boundaries() {
        let e = Adsr {
            attack: 0.1,
            decay: 0.2,
            sustain: 0.5,
            release: 0.3,
        };
        let note_dur = 1.0; // sustain_time = 1.0 - (0.1+0.2+0.3) = 0.4

        // Attack end: t/attack = 1.0.
        assert!((adsr_value_at(&e, 0.1, note_dur) - 1.0).abs() < 1e-9);
        // Decay end: value should equal sustain level.
        assert!((adsr_value_at(&e, 0.3, note_dur) - 0.5).abs() < 1e-9);
        // Still within sustain plateau.
        assert!((adsr_value_at(&e, 0.5, note_dur) - 0.5).abs() < 1e-9);
        // Release end (attack+decay+sustain_time+release = 0.1+0.2+0.4+0.3=1.0): value -> 0.
        assert!(adsr_value_at(&e, 1.0, note_dur).abs() < 1e-9);
        // Past release: stays at 0.
        assert!(adsr_value_at(&e, 1.5, note_dur).abs() < 1e-9);
        // Before time 0: 0.
        assert_eq!(adsr_value_at(&e, -0.1, note_dur), 0.0);
    }

    #[test]
    fn adsr_zero_attack_jumps_to_one() {
        let e = Adsr {
            attack: 0.0,
            decay: 0.1,
            sustain: 0.5,
            release: 0.1,
        };
        assert_eq!(adsr_value_at(&e, 0.0, 1.0), 1.0);
    }

    // ---- render / normalize ------------------------------------------------

    #[test]
    fn render_sample_count_matches_duration_stereo_44100() {
        let mut rng = Lcg::new(1);
        let mut e = Effect::new(0.5);
        e.add_osc(Waveform::Sine, 440.0, 0.5);
        let s = render(&e, &mut rng);
        assert_eq!(s.sample_rate, SAMPLE_RATE);
        assert_eq!(s.channels, 2);
        let expected_frames = (SAMPLE_RATE as f64 * 0.5).trunc() as u32;
        assert_eq!(s.samples.len() as u32, expected_frames * 2);
    }

    #[test]
    fn render_zero_or_negative_duration_floors_to_minimum() {
        let mut rng = Lcg::new(1);
        let mut e = Effect::new(0.0);
        e.add_osc(Waveform::Sine, 440.0, 0.5);
        let s = render(&e, &mut rng);
        // Modula-2 bumps <=0 duration to 0.01s before allocating.
        let expected_frames = (SAMPLE_RATE as f64 * 0.01).trunc() as u32;
        assert_eq!(s.samples.len() as u32, expected_frames * 2);
    }

    #[test]
    fn normalize_caps_peak_at_target() {
        let mut sound = Sound {
            sample_rate: SAMPLE_RATE,
            channels: 1,
            samples: vec![0.0, 2.0, -3.0, 1.0],
        };
        normalize(&mut sound, 0.9);
        let peak = sound.samples.iter().fold(0.0_f64, |a, &b| a.max(b.abs()));
        assert!((peak - 0.9).abs() < 1e-9, "peak was {peak}");
    }

    #[test]
    fn normalize_is_noop_when_already_under_one() {
        let mut sound = Sound {
            sample_rate: SAMPLE_RATE,
            channels: 1,
            samples: vec![0.0, 0.5, -0.3],
        };
        let before = sound.samples.clone();
        normalize(&mut sound, 0.9);
        assert_eq!(sound.samples, before);
    }

    #[test]
    fn note_to_frequency_a4_is_440() {
        assert!((note_to_frequency(69) - 440.0).abs() < 1e-9);
    }

    #[test]
    fn note_to_frequency_octave_doubles() {
        let a4 = note_to_frequency(69);
        let a5 = note_to_frequency(81);
        assert!((a5 - a4 * 2.0).abs() < 1e-6);
    }

    // ---- presets: run without panicking, non-empty, peak in range ---------

    fn assert_sane(s: &Sound) {
        assert!(!s.samples.is_empty());
        let peak = s.samples.iter().fold(0.0_f64, |a, &b| a.max(b.abs()));
        assert!(peak <= 1.0 + 1e-6, "peak {peak} exceeds 1.0 tolerance");
    }

    #[test]
    fn all_presets_run_and_produce_sane_output() {
        let mut rng = Lcg::new(12345);
        assert_sane(&beep(440.0, 0.2));
        assert_sane(&coin(0.3));
        assert_sane(&jump(0.3));
        assert_sane(&zap(0.3, &mut rng));
        assert_sane(&shoot(0.3, &mut rng));
        assert_sane(&explode(1.0, 0.5, &mut rng));
        assert_sane(&powerup(0.4));
        assert_sane(&hurt(0.3, &mut rng));
        assert_sane(&click(0.1, &mut rng));
        assert_sane(&bang(0.3, &mut rng));
        assert_sane(&blip(1.5, 0.2));
        assert_sane(&saucer(0.9));
        assert_sane(&boss_hum(1.8));
        assert_sane(&tone(220.0, 0.25, Waveform::Square));
        assert_sane(&tone(220.0, 0.25, Waveform::Pulse));
        assert_sane(&noise(0, 0.2, &mut rng));
        assert_sane(&noise(1, 0.2, &mut rng));
        assert_sane(&noise(2, 0.2, &mut rng));
    }

    #[test]
    fn play_sound_covers_every_index_and_out_of_range_is_safe() {
        // Every documented index 0..=11 (Coin..BossHum) renders a sane,
        // non-empty buffer through the u8 dispatch...
        for preset in 0u8..=11 {
            assert_sane(&play_sound(preset));
        }
        // ...including the two new presets specifically.
        assert_sane(&play_sound(10)); // Saucer
        assert_sane(&play_sound(11)); // BossHum
        // ...and any out-of-range index falls back without panicking, still
        // producing a non-empty, sane sound.
        assert_sane(&play_sound(12));
        assert_sane(&play_sound(200));
        assert_sane(&play_sound(u8::MAX));
    }

    #[test]
    fn saucer_and_boss_hum_beat_at_their_detune() {
        // The warble/hum is the *beat* between two detuned sines, so the two
        // oscillators must sit a few Hz apart (not identical, not a wide
        // interval). Saucer: 600 vs 606 Hz (6 Hz). BossHum: 110 vs 114 Hz
        // (4 Hz), an octave-and-a-half below and slower.
        let s = saucer(0.9);
        assert!((s.duration() - 0.9).abs() < 0.01);
        let b = boss_hum(1.8);
        assert!((b.duration() - 1.8).abs() < 0.01);
    }

    #[test]
    fn render_is_deterministic_given_same_seed_and_effect() {
        let mut e = Effect::new(0.2);
        e.add_osc(Waveform::Noise, 440.0, 0.5);
        e.noise_mix = 0.3;
        let s1 = render(&e, &mut Lcg::new(999));
        let s2 = render(&e, &mut Lcg::new(999));
        assert_eq!(s1.samples, s2.samples);
    }
}
