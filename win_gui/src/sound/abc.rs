//! ABC-notation -> timed MIDI events, ported from MacModula2's
//! `Abc.mod`/`Abc.def`.
//!
//! Parses ABC music text (headers, inline fields, `%%MIDI` directives,
//! key-signature-aware accidentals persistent within a bar, notes/octaves/
//! durations including fractions and dots, ties, broken rhythm, tuplets,
//! chords, rests, bar-line accidental reset, single-line-only `|: :|` repeat
//! expansion, and multi-voice `V:` switching) into a flat, time-sorted list
//! of [`MidiEvent`]s with absolute millisecond timestamps, so a player does
//! zero music math at play time.
//!
//! Pitch convention follows ABC: uppercase `C` = MIDI 48, lowercase `c` =
//! MIDI 60.
//!
//! All of the Modula-2 original's module-level mutable globals (`gSrc`,
//! `gCurMs`, `gKeySharps`, the per-voice save/restore arrays, ...) are
//! collected here into one `Parser` struct threaded through the private
//! helper functions instead — the algorithm is unchanged, only the state's
//! home has moved from module statics to a value passed by `&mut self`.

const VELOCITY: u8 = 80;
const MAX_VOICES: usize = 16;

/// One timed MIDI event (`Abc.def`'s `MidiEvent`). `status` is the raw
/// status byte without the channel nibble folded in (0x90 NoteOn, 0x80
/// NoteOff, 0xC0 ProgramChange, 0xB0 ControlChange).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiEvent {
    pub time_ms: u32,
    pub status: u8,
    pub chan: u8,
    pub d1: u8,
    pub d2: u8,
}

/// A parsed tune: a time-sorted event list plus the parsed tempo and the
/// time of the last event (`Abc.def`'s `Tune`).
#[derive(Debug, Clone, Default)]
pub struct Tune {
    pub events: Vec<MidiEvent>,
    pub bpm: u32,
    pub end_ms: u32,
}

// ---- char helpers (mirrors Abc.mod's Up/IsDigit/IsAlpha/IsNote/IsBar/IsAccM) --

fn up(c: u8) -> u8 {
    if c.is_ascii_lowercase() {
        c - 32
    } else {
        c
    }
}
fn lower(c: u8) -> u8 {
    if c.is_ascii_uppercase() {
        c + 32
    } else {
        c
    }
}
fn is_digit(c: u8) -> bool {
    c.is_ascii_digit()
}
fn is_alpha(c: u8) -> bool {
    c.is_ascii_alphabetic()
}
fn is_note(c: u8) -> bool {
    (b'A'..=b'G').contains(&c) || (b'a'..=b'g').contains(&c)
}
fn is_bar(c: u8) -> bool {
    c == b'|' || c == b':' || c == b'[' || c == b']'
}
fn is_acc_marker(c: u8) -> bool {
    c == b'^' || c == b'_' || c == b'='
}

fn round_ms(x: f64) -> u32 {
    if x < 0.0 {
        0
    } else {
        (x + 0.5).trunc() as u32
    }
}

/// Per-voice saved state (`Abc.mod`'s `VoiceState`): everything a working
/// voice needs to suspend into / resume from when `V:` switches away and
/// back.
#[derive(Debug, Clone)]
struct VoiceState {
    name: String,
    channel: i32,
    instrument: i32,
    cur_ms: f64,
    key_sharps: i32,
    unit_n: i32,
    unit_d: i32,
    ts_n: i32,
    ts_d: i32,
    transpose: i32,
    bar_acc: [i32; 7],
    bar_set: [bool; 7],
}

/// All parser working state, collected from `Abc.mod`'s module-level `g*`
/// globals into one struct threaded by `&mut self` through the helpers.
struct Parser {
    tune: Tune,
    in_body: bool,
    cur_ms: f64,
    ms_per_whole: f64,
    bpm: i32,
    unit_n: i32,
    unit_d: i32,
    ts_n: i32,
    ts_d: i32,
    key_sharps: i32,
    transpose: i32,
    chan: i32,
    instrument: i32,
    bar_acc: [i32; 7],
    bar_set: [bool; 7],
    tup_n: i32,
    tup_d: i32,
    tup_rem: i32,
    voices: Vec<VoiceState>,
    cur: i32,
    next_chan: i32,
    def_key: i32,
    def_unit_n: i32,
    def_unit_d: i32,
    def_ts_n: i32,
    def_ts_d: i32,
    def_instr: i32,
}

impl Parser {
    fn new() -> Self {
        let voices = vec![VoiceState {
            name: "1".to_string(),
            channel: 0,
            instrument: 0,
            cur_ms: 0.0,
            key_sharps: 0,
            unit_n: 1,
            unit_d: 8,
            ts_n: 4,
            ts_d: 4,
            transpose: 0,
            bar_acc: [0; 7],
            bar_set: [false; 7],
        }];
        Parser {
            tune: Tune::default(),
            in_body: false,
            cur_ms: 0.0,
            ms_per_whole: 2000.0,
            bpm: 120,
            unit_n: 1,
            unit_d: 8,
            ts_n: 4,
            ts_d: 4,
            key_sharps: 0,
            transpose: 0,
            chan: 0,
            instrument: 0,
            bar_acc: [0; 7],
            bar_set: [false; 7],
            tup_n: 1,
            tup_d: 1,
            tup_rem: 0,
            voices,
            cur: 0,
            next_chan: 1,
            def_key: 0,
            def_unit_n: 1,
            def_unit_d: 8,
            def_ts_n: 4,
            def_ts_d: 4,
            def_instr: 0,
        }
    }

    // ---- event output --------------------------------------------------

    fn push(&mut self, time_ms: u32, status: u8, chan: u8, d1: u8, d2: u8) {
        const MAX_EVENTS: usize = 16384;
        if self.tune.events.len() < MAX_EVENTS {
            self.tune.events.push(MidiEvent {
                time_ms,
                status,
                chan,
                d1,
                d2,
            });
        }
    }

    fn emit_note(&mut self, midi: i32, dur_wholes: f64) {
        let start_ms = round_ms(self.cur_ms);
        let end_ms = round_ms(self.cur_ms + dur_wholes * self.ms_per_whole);
        let chan = self.chan as u8;
        let pitch = midi as u8;
        self.push(start_ms, 0x90, chan, pitch, VELOCITY);
        self.push(end_ms, 0x80, chan, pitch, 0);
    }

    // ---- pitch / duration -----------------------------------------------

    fn key_accidental(&self, p_upper: u8) -> i32 {
        if self.key_sharps > 0 && self.key_sharps >= sharp_pos(p_upper) {
            return 1;
        }
        if self.key_sharps < 0 && (-self.key_sharps) >= flat_pos(p_upper) {
            return -1;
        }
        0
    }

    /// Parse a note at `pos`; on success returns `(midi, dn, dd)` and
    /// advances `pos` past it; updates bar accidentals (`ParseNote`).
    fn parse_note(&mut self, ln: &[u8], pos: &mut usize) -> Option<(i32, i32, i32)> {
        let len = ln.len();
        if *pos >= len {
            return None;
        }
        let mut probe = *pos;
        let _explicit_probe = parse_acc(ln, &mut probe);
        if probe >= len || !is_note(ln[probe]) {
            return None;
        }
        let explicit = parse_acc(ln, pos);
        let p = ln[*pos];
        *pos += 1;
        let pu = up(p);
        let lower_case = p.is_ascii_lowercase();
        let mut octave: i32 = 0;
        while *pos < len && ln[*pos] == b'\'' {
            octave += 1;
            *pos += 1;
        }
        while *pos < len && ln[*pos] == b',' {
            octave -= 1;
            *pos += 1;
        }
        let (dn, dd) = self.parse_dur(ln, pos);
        let idx = pitch_index(pu);
        let mut acc = self.key_accidental(pu);
        if let Some(explicit) = explicit {
            acc = explicit;
            if idx >= 0 {
                self.bar_acc[idx as usize] = acc;
                self.bar_set[idx as usize] = true;
            }
        } else if idx >= 0 && self.bar_set[idx as usize] {
            acc = self.bar_acc[idx as usize];
        }
        let semi = match pu {
            b'A' => 9,
            b'B' => 11,
            b'C' => 0,
            b'D' => 2,
            b'E' => 4,
            b'F' => 5,
            b'G' => 7,
            _ => 0,
        };
        let base = if lower_case { 5 } else { 4 };
        let midi = (base * 12 + semi + acc + octave * 12 + self.transpose).clamp(0, 127);
        Some((midi, dn, dd))
    }

    /// Duration in whole-note units, defaulting to the current unit length
    /// (`ParseDur`).
    fn parse_dur(&self, ln: &[u8], pos: &mut usize) -> (i32, i32) {
        let len = ln.len();
        let (mut dn, mut dd) = (self.unit_n, self.unit_d);
        if *pos < len && is_digit(ln[*pos]) {
            let mut numer: i32 = 0;
            while *pos < len && is_digit(ln[*pos]) {
                numer = numer * 10 + (ln[*pos] - b'0') as i32;
                *pos += 1;
            }
            if *pos < len && ln[*pos] == b'/' {
                *pos += 1;
                let mut denom: i32 = 2;
                if *pos < len && is_digit(ln[*pos]) {
                    denom = 0;
                    while *pos < len && is_digit(ln[*pos]) {
                        denom = denom * 10 + (ln[*pos] - b'0') as i32;
                        *pos += 1;
                    }
                }
                dn = numer * self.unit_n;
                dd = denom * self.unit_d;
            } else {
                dn = numer * self.unit_n;
                dd = self.unit_d;
            }
        } else if *pos < len && ln[*pos] == b'/' {
            *pos += 1;
            let mut denom: i32 = 2;
            if *pos < len && is_digit(ln[*pos]) {
                denom = 0;
                while *pos < len && is_digit(ln[*pos]) {
                    denom = denom * 10 + (ln[*pos] - b'0') as i32;
                    *pos += 1;
                }
            }
            dn = self.unit_n;
            dd = self.unit_d * denom;
        }
        while *pos < len && ln[*pos] == b'.' {
            dn *= 3;
            dd *= 2;
            *pos += 1;
        }
        (dn, dd)
    }

    fn reset_bar(&mut self) {
        self.bar_set = [false; 7];
    }

    fn apply_tuplet(&mut self, dn: &mut i32, dd: &mut i32) {
        if self.tup_rem > 0 {
            *dn *= self.tup_n;
            *dd *= self.tup_d;
            self.tup_rem -= 1;
            if self.tup_rem <= 0 {
                self.tup_n = 1;
                self.tup_d = 1;
            }
        }
    }

    // ---- the note sequence loop (ParseSeq, single voice) -----------------

    fn parse_seq(&mut self, ln: &[u8]) {
        let len = ln.len();
        let mut pos = 0usize;
        while pos < len {
            while pos < len && (ln[pos] == b' ' || ln[pos] == b'\t') {
                pos += 1;
            }
            if pos >= len {
                return;
            }

            if ln[pos] == b'{' {
                skip_grace(ln, &mut pos);
            } else if ln[pos] == b'['
                && pos + 2 < len
                && is_alpha(ln[pos + 1])
                && ln[pos + 2] == b':'
            {
                let start = pos + 3;
                pos += 3;
                if up(ln[start - 2]) == b'V' {
                    let mut ie = pos;
                    while ie < len && ln[ie] != b']' && ln[ie] != b' ' && ln[ie] != b'\t' {
                        ie += 1;
                    }
                    while pos < len && ln[pos] != b']' {
                        pos += 1;
                    }
                    self.switch_voice(ln, start, ie);
                    if pos < len {
                        pos += 1;
                    }
                } else {
                    while pos < len && ln[pos] != b']' {
                        pos += 1;
                    }
                    self.apply_inline(up(ln[start - 2]), ln, start, pos);
                    if pos < len {
                        pos += 1;
                    }
                }
            } else if ln[pos] == b'"' {
                pos += 1; // guitar chord: skip "..." (silent)
                while pos < len && ln[pos] != b'"' {
                    pos += 1;
                }
                if pos < len {
                    pos += 1;
                }
            } else if ln[pos] == b'[' {
                // chord [CEG]
                let mut cmidi: Vec<i32> = Vec::with_capacity(16);
                pos += 1;
                while pos < len && ln[pos] != b']' {
                    while pos < len && (ln[pos] == b' ' || ln[pos] == b'\t') {
                        pos += 1;
                    }
                    if pos < len && ln[pos] != b']' {
                        if let Some((nmidi, _ndn, _ndd)) = self.parse_note(ln, &mut pos) {
                            if cmidi.len() <= 15 {
                                cmidi.push(nmidi);
                            }
                        } else {
                            pos += 1;
                        }
                    }
                }
                if pos < len && ln[pos] == b']' {
                    pos += 1;
                    let (dn, dd) = self.parse_dur(ln, &mut pos);
                    if !cmidi.is_empty() {
                        let dur_w = dn as f64 / dd as f64;
                        for &m in &cmidi {
                            self.emit_note(m, dur_w);
                        }
                        self.cur_ms += dur_w * self.ms_per_whole;
                    }
                }
            } else if is_bar(ln[pos]) {
                while pos < len && is_bar(ln[pos]) {
                    pos += 1;
                }
                self.reset_bar();
            } else if ln[pos] == b'z' || ln[pos] == b'Z' {
                pos += 1;
                let (dn, dd) = self.parse_dur(ln, &mut pos);
                self.cur_ms += (dn as f64 / dd as f64) * self.ms_per_whole;
            } else if ln[pos] == b'(' {
                if pos + 1 < len && is_digit(ln[pos + 1]) {
                    self.parse_tuplet(ln, &mut pos);
                } else {
                    pos += 1;
                }
            } else if ln[pos] == b')' {
                pos += 1;
            } else if is_note(ln[pos]) || is_acc_marker(ln[pos]) {
                if let Some((midi, mut dn, mut dd)) = self.parse_note(ln, &mut pos) {
                    let mut has_broken = false;
                    let mut broken_longer = false;
                    if pos < len {
                        if ln[pos] == b'>' {
                            has_broken = true;
                            broken_longer = true;
                            pos += 1;
                        } else if ln[pos] == b'<' {
                            has_broken = true;
                            pos += 1;
                        }
                    }
                    if has_broken {
                        if broken_longer {
                            dn *= 3;
                            dd *= 2;
                        } else {
                            dd *= 2;
                        }
                    }
                    self.apply_tuplet(&mut dn, &mut dd);
                    if pos < len && ln[pos] == b'-' {
                        pos += 1;
                        self.merge_tie(ln, &mut pos, midi, &mut dn, &mut dd);
                    }
                    let dur_w = dn as f64 / dd as f64;
                    self.emit_note(midi, dur_w);
                    self.cur_ms += dur_w * self.ms_per_whole;
                    if has_broken {
                        while pos < len && (ln[pos] == b' ' || ln[pos] == b'\t') {
                            pos += 1;
                        }
                        if pos < len && is_note(ln[pos]) {
                            if let Some((nmidi, mut ndn, mut ndd)) = self.parse_note(ln, &mut pos) {
                                if broken_longer {
                                    ndd *= 2;
                                } else {
                                    ndn *= 3;
                                    ndd *= 2;
                                }
                                self.apply_tuplet(&mut ndn, &mut ndd);
                                if pos < len && ln[pos] == b'-' {
                                    pos += 1;
                                    self.merge_tie(ln, &mut pos, nmidi, &mut ndn, &mut ndd);
                                }
                                let dur_w = ndn as f64 / ndd as f64;
                                self.emit_note(nmidi, dur_w);
                                self.cur_ms += dur_w * self.ms_per_whole;
                            }
                        }
                    }
                } else {
                    pos += 1;
                }
            } else {
                pos += 1;
            }
        }
    }

    /// Tie merge: while tied, sum durations of same-pitch following notes
    /// (`MergeTie`).
    fn merge_tie(&mut self, ln: &[u8], pos: &mut usize, midi: i32, dn: &mut i32, dd: &mut i32) {
        let len = ln.len();
        let mut tied = true;
        while tied {
            let mut scan = *pos;
            while scan < len && matches!(ln[scan], b' ' | b'\t' | 13 | b'|' | b':') {
                scan += 1;
            }
            if scan >= len || !is_note(ln[scan]) {
                tied = false;
            } else if let Some((nmidi, ndn, ndd)) = self.parse_note(ln, &mut scan) {
                if nmidi != midi {
                    tied = false;
                } else {
                    *dn = *dn * ndd + ndn * *dd; // fraction add
                    *dd *= ndd;
                    *pos = scan;
                    if *pos < len && ln[*pos] == b'-' {
                        *pos += 1;
                    } else {
                        tied = false;
                    }
                }
            } else {
                tied = false;
            }
        }
    }

    fn parse_tuplet(&mut self, ln: &[u8], pos: &mut usize) {
        let len = ln.len();
        *pos += 1; // '('
        let p = (ln[*pos] - b'0') as i32;
        *pos += 1;
        let mut q: i32 = -1;
        let mut r: i32 = -1;
        if *pos < len && ln[*pos] == b':' {
            *pos += 1;
            if *pos < len && is_digit(ln[*pos]) {
                q = (ln[*pos] - b'0') as i32;
                *pos += 1;
            }
            if *pos < len && ln[*pos] == b':' {
                *pos += 1;
                if *pos < len && is_digit(ln[*pos]) {
                    r = (ln[*pos] - b'0') as i32;
                    *pos += 1;
                }
            }
        }
        let compound = self.ts_d == 8 && (self.ts_n == 6 || self.ts_n == 9 || self.ts_n == 12);
        let inferred_q = match p {
            2 => 3,
            3 => 2,
            4 => 3,
            6 => 2,
            8 => 3,
            _ => {
                if compound {
                    3
                } else {
                    2
                }
            }
        };
        if q < 0 {
            q = inferred_q;
        }
        if r < 0 {
            r = p;
        }
        if p > 0 && q > 0 && r > 0 {
            self.tup_n = q;
            self.tup_d = p;
            self.tup_rem = r;
        }
    }

    // ---- headers / inline -------------------------------------------------

    fn recompute(&mut self) {
        self.ms_per_whole = self.ts_d as f64 * 60000.0 / self.bpm as f64;
    }

    /// Apply an inline field K/Q/M/L in the body (`s[from..to)` is the
    /// value) (`ApplyInline`).
    fn apply_inline(&mut self, field: u8, s: &[u8], from: usize, to: usize) {
        match field {
            b'K' => self.key_sharps = parse_key(s, from, to),
            b'Q' => {
                self.bpm = parse_bpm(s, from, to, self.bpm);
                self.recompute();
            }
            b'M' => {
                parse_meter(s, from, to, &mut self.ts_n, &mut self.ts_d);
                self.recompute();
            }
            b'L' => parse_frac(s, from, to, &mut self.unit_n, &mut self.unit_d),
            _ => {}
        }
    }

    fn parse_header(&mut self, s: &[u8]) {
        let n = s.len();
        let field = up(s[0]);
        let mut from = 2usize;
        while from < n && (s[from] == b' ' || s[from] == b'\t') {
            from += 1;
        }
        match field {
            b'M' => parse_meter(s, from, n, &mut self.ts_n, &mut self.ts_d),
            b'L' => parse_frac(s, from, n, &mut self.unit_n, &mut self.unit_d),
            b'Q' => self.bpm = parse_bpm(s, from, n, self.bpm),
            b'K' => self.key_sharps = parse_key(s, from, n),
            _ => {}
        }
    }

    /// `%%MIDI` program/channel/transpose/drum -> apply to the current
    /// voice (`ParseMidiDir`).
    fn parse_midi_dir(&mut self, s: &[u8]) {
        let n = s.len();
        if n < 6 {
            return;
        }
        let mut i = 6usize;
        while i < n && (s[i] == b' ' || s[i] == b'\t') {
            i += 1;
        }
        if i >= n {
            return;
        }
        let c0 = lower(s[i]);
        let c1 = if i + 1 < n { lower(s[i + 1]) } else { b' ' };
        while i < n && s[i] != b' ' && s[i] != b'\t' {
            i += 1;
        }
        while i < n && (s[i] == b' ' || s[i] == b'\t') {
            i += 1;
        }
        let mut neg = false;
        if i < n && s[i] == b'-' {
            neg = true;
            i += 1;
        }
        let mut num: i32 = 0;
        while i < n && is_digit(s[i]) {
            num = num * 10 + (s[i] - b'0') as i32;
            i += 1;
        }
        if c0 == b'p' && c1 == b'r' {
            if num <= 127 {
                self.instrument = num;
            }
        } else if c0 == b'c' && c1 == b'h' {
            if (1..=16).contains(&num) {
                self.chan = num - 1;
            }
        } else if c0 == b't' {
            self.transpose = if neg { -num } else { num };
        } else if c0 == b'd' || c0 == b'p' {
            self.chan = 9;
        }
    }

    // ---- voices -----------------------------------------------------------

    fn save_cur(&mut self) {
        if self.cur >= 0 && (self.cur as usize) < self.voices.len() {
            let v = &mut self.voices[self.cur as usize];
            v.cur_ms = self.cur_ms;
            v.key_sharps = self.key_sharps;
            v.unit_n = self.unit_n;
            v.unit_d = self.unit_d;
            v.ts_n = self.ts_n;
            v.ts_d = self.ts_d;
            v.transpose = self.transpose;
            v.channel = self.chan;
            v.instrument = self.instrument;
            v.bar_acc = self.bar_acc;
            v.bar_set = self.bar_set;
        }
    }

    fn load_cur(&mut self) {
        let v = &self.voices[self.cur as usize];
        self.cur_ms = v.cur_ms;
        self.key_sharps = v.key_sharps;
        self.unit_n = v.unit_n;
        self.unit_d = v.unit_d;
        self.ts_n = v.ts_n;
        self.ts_d = v.ts_d;
        self.transpose = v.transpose;
        self.chan = v.channel;
        self.instrument = v.instrument;
        self.bar_acc = v.bar_acc;
        self.bar_set = v.bar_set;
        self.recompute();
    }

    fn assign_channel(&mut self) -> i32 {
        let mut ch = self.next_chan;
        if ch == 9 {
            ch = 10;
        }
        if ch > 15 {
            ch = 15;
        }
        self.next_chan = ch + 1;
        if self.next_chan == 9 {
            self.next_chan = 10;
        }
        ch
    }

    fn find_voice(&self, ident: &[u8], from: usize, to: usize) -> i32 {
        let target = &ident[from..to];
        for (i, v) in self.voices.iter().enumerate() {
            if v.name.as_bytes() == target {
                return i as i32;
            }
        }
        -1
    }

    fn create_voice(&mut self, ident: &[u8], from: usize, to: usize) -> i32 {
        if self.voices.len() >= MAX_VOICES {
            return self.cur;
        }
        let idx = self.voices.len() as i32;
        let name = String::from_utf8_lossy(&ident[from..to.min(from + 15)]).to_string();
        let channel = self.assign_channel();
        self.voices.push(VoiceState {
            name,
            channel,
            instrument: self.def_instr,
            cur_ms: 0.0,
            key_sharps: self.def_key,
            unit_n: self.def_unit_n,
            unit_d: self.def_unit_d,
            ts_n: self.def_ts_n,
            ts_d: self.def_ts_d,
            transpose: 0,
            bar_acc: [0; 7],
            bar_set: [false; 7],
        });
        idx
    }

    fn switch_voice(&mut self, ident: &[u8], from: usize, to: usize) {
        if to <= from {
            return;
        }
        self.save_cur();
        let mut i = self.find_voice(ident, from, to);
        if i < 0 {
            i = self.create_voice(ident, from, to);
        }
        self.cur = i;
        self.load_cur();
    }

    fn handle_voice_line(&mut self, ln: &[u8]) {
        let n = ln.len();
        let mut i = 2usize;
        while i < n && (ln[i] == b' ' || ln[i] == b'\t') {
            i += 1;
        }
        let id_start = i;
        while i < n && ln[i] != b' ' && ln[i] != b'\t' {
            i += 1;
        }
        let id_end = i;
        self.switch_voice(ln, id_start, id_end);
    }

    fn snapshot_defaults(&mut self) {
        self.def_key = self.key_sharps;
        self.def_unit_n = self.unit_n;
        self.def_unit_d = self.unit_d;
        self.def_ts_n = self.ts_n;
        self.def_ts_d = self.ts_d;
        self.def_instr = self.instrument;
    }

    // ---- per-line dispatch -------------------------------------------------

    fn process_line(&mut self, line: &[u8]) {
        let ln = trim_end(line);
        let n = ln.len();
        if n == 0 {
            return;
        }
        if n >= 6 && ln[0] == b'%' && ln[1] == b'%' && up(ln[2]) == b'M' && up(ln[3]) == b'I' {
            self.parse_midi_dir(ln);
            return;
        }
        if ln[0] == b'%' {
            return;
        }
        if n >= 2 && up(ln[0]) == b'V' && ln[1] == b':' {
            self.handle_voice_line(ln);
            return;
        }
        if n >= 2 && is_alpha(ln[0]) && ln[1] == b':' {
            if self.in_body {
                self.apply_inline(up(ln[0]), ln, 2, n);
            } else {
                self.parse_header(ln);
                if up(ln[0]) == b'K' {
                    self.in_body = true;
                    self.snapshot_defaults();
                    self.recompute();
                }
            }
            return;
        }
        if !self.in_body {
            self.in_body = true;
            self.snapshot_defaults();
            self.recompute();
        }
        let expanded = expand_repeat(ln);
        self.parse_seq(&expanded);
    }

    fn sort_events(&mut self) {
        fn pri(status: u8) -> u32 {
            match status {
                0x80 => 1,
                0xB0 => 2,
                0xC0 => 3,
                _ => 4,
            }
        }
        // Stable sort by (time_ms, priority) — insertion order preserved
        // among equal keys, matching the Modula-2 insertion sort (which is
        // stable) (`Sort`).
        self.tune
            .events
            .sort_by_key(|e| e.time_ms as u64 * 16 + pri(e.status) as u64);
    }

    fn finish(mut self) -> Tune {
        self.save_cur(); // flush current voice's instrument
        for v in &self.voices {
            self.tune.events.push(MidiEvent {
                time_ms: 0,
                status: 0xC0,
                chan: v.channel as u8,
                d1: v.instrument as u8,
                d2: 0,
            });
        }
        self.sort_events();
        let max_end = self
            .tune
            .events
            .iter()
            .map(|e| e.time_ms)
            .max()
            .unwrap_or(0);
        self.tune.bpm = self.bpm as u32;
        self.tune.end_ms = max_end;
        self.tune
    }
}

// ---- free helper functions (module-level in the Modula-2 source) ---------

/// A..G -> 0..6, else -1 (`PitchIndex`).
fn pitch_index(p_upper: u8) -> i32 {
    match p_upper {
        b'A' => 0,
        b'B' => 1,
        b'C' => 2,
        b'D' => 3,
        b'E' => 4,
        b'F' => 5,
        b'G' => 6,
        _ => -1,
    }
}

/// Sharp order F C G D A E B -> F=1..B=7 (`SharpPos`).
fn sharp_pos(pu: u8) -> i32 {
    match pu {
        b'F' => 1,
        b'C' => 2,
        b'G' => 3,
        b'D' => 4,
        b'A' => 5,
        b'E' => 6,
        b'B' => 7,
        _ => 99,
    }
}

/// Flat order B E A D G C F -> B=1..F=7 (`FlatPos`).
fn flat_pos(pu: u8) -> i32 {
    match pu {
        b'B' => 1,
        b'E' => 2,
        b'A' => 3,
        b'D' => 4,
        b'G' => 5,
        b'C' => 6,
        b'F' => 7,
        _ => 99,
    }
}

/// Parse a leading accidental marker (`^`, `^^`, `_`, `__`, `=`), advancing
/// `pos` past it. Returns `None` if there is none (`ParseAcc`; `-99` in the
/// source becomes `None` here).
fn parse_acc(ln: &[u8], pos: &mut usize) -> Option<i32> {
    let len = ln.len();
    if *pos >= len {
        return None;
    }
    match ln[*pos] {
        b'^' => {
            if *pos + 1 < len && ln[*pos + 1] == b'^' {
                *pos += 2;
                Some(2)
            } else {
                *pos += 1;
                Some(1)
            }
        }
        b'_' => {
            if *pos + 1 < len && ln[*pos + 1] == b'_' {
                *pos += 2;
                Some(-2)
            } else {
                *pos += 1;
                Some(-1)
            }
        }
        b'=' => {
            *pos += 1;
            Some(0)
        }
        _ => None,
    }
}

fn skip_grace(ln: &[u8], pos: &mut usize) -> bool {
    let len = ln.len();
    if *pos >= len || ln[*pos] != b'{' {
        return false;
    }
    *pos += 1;
    while *pos < len && ln[*pos] != b'}' {
        *pos += 1;
    }
    if *pos < len {
        *pos += 1;
    }
    true
}

fn trim_end(s: &[u8]) -> &[u8] {
    let mut n = s.len();
    while n > 0 && matches!(s[n - 1], b' ' | b'\t' | 13) {
        n -= 1;
    }
    &s[..n]
}

fn parse_meter(s: &[u8], from: usize, to: usize, ts_n: &mut i32, ts_d: &mut i32) {
    if from < to && up(s[from]) == b'C' {
        if from + 1 < to && s[from + 1] == b'|' {
            *ts_n = 2;
            *ts_d = 2;
        } else {
            *ts_n = 4;
            *ts_d = 4;
        }
        return;
    }
    let (mut num, mut den) = (0i32, 0i32);
    let mut slash = false;
    for &c in &s[from..to] {
        if c == b'/' {
            slash = true;
        } else if is_digit(c) {
            if slash {
                den = den * 10 + (c - b'0') as i32;
            } else {
                num = num * 10 + (c - b'0') as i32;
            }
        }
    }
    if num > 0 && den > 0 {
        *ts_n = num;
        *ts_d = den;
    }
}

fn parse_frac(s: &[u8], from: usize, to: usize, n: &mut i32, d: &mut i32) {
    let (mut num, mut den) = (0i32, 0i32);
    let mut slash = false;
    for &c in &s[from..to] {
        if c == b'/' {
            slash = true;
        } else if is_digit(c) {
            if slash {
                den = den * 10 + (c - b'0') as i32;
            } else {
                num = num * 10 + (c - b'0') as i32;
            }
        }
    }
    if num > 0 && den > 0 {
        *n = num;
        *d = den;
    }
}

fn parse_bpm(s: &[u8], from: usize, to: usize, current: i32) -> i32 {
    let mut start = from; // after the last '='
    for (i, &c) in s.iter().enumerate().take(to).skip(from) {
        if c == b'=' {
            start = i + 1;
        }
    }
    let mut num: i32 = 0;
    for &c in &s[start..to] {
        if is_digit(c) {
            num = num * 10 + (c - b'0') as i32;
        }
    }
    if num == 0 {
        current
    } else {
        num
    }
}

fn parse_key(s: &[u8], from: usize, to: usize) -> i32 {
    if from >= to {
        return 0;
    }
    let c0 = s[from];
    let c1 = if from + 1 < to { s[from + 1] } else { b' ' };
    if c0 == b'C' && c1 == b'#' {
        return 7;
    }
    if c0 == b'F' && c1 == b'#' {
        return 6;
    }
    if c0 == b'C' && c1 == b'b' {
        return -7;
    }
    if c0 == b'G' && c1 == b'b' {
        return -6;
    }
    if c0 == b'D' && c1 == b'b' {
        return -5;
    }
    if c0 == b'A' && c1 == b'b' {
        return -4;
    }
    if c0 == b'E' && c1 == b'b' {
        return -3;
    }
    if c0 == b'B' && c1 == b'b' {
        return -2;
    }
    match c0 {
        b'B' => 5,
        b'E' => 4,
        b'A' => 3,
        b'D' => 2,
        b'G' => 1,
        b'F' => -1,
        _ => 0,
    }
}

/// Single-line `|: inner :|` -> `inner inner` (first occurrence only); a
/// no-op if no complete `|: ... :|` pair is found on this line
/// (`ExpandRepeat`). Deliberately single-line-only — matching the source's
/// documented limitation, not a bug to fix.
fn expand_repeat(src: &[u8]) -> Vec<u8> {
    let n = src.len();
    let mut i = 0usize;
    let mut found = false;
    while i + 1 < n && !found {
        if src[i] == b'|' && src[i + 1] == b':' {
            found = true;
        } else {
            i += 1;
        }
    }
    if !found {
        return src.to_vec();
    }
    let s2 = i + 2;
    let mut e2 = s2;
    while e2 + 1 < n && !(src[e2] == b':' && src[e2 + 1] == b'|') {
        e2 += 1;
    }
    if e2 + 1 >= n || !(src[e2] == b':' && src[e2 + 1] == b'|') {
        return src.to_vec();
    }
    let mut dst = Vec::with_capacity(n + (e2 - s2) + 1);
    dst.extend_from_slice(&src[..i]); // before |:
    dst.extend_from_slice(&src[s2..e2]); // inner
    dst.push(b' ');
    dst.extend_from_slice(&src[s2..e2]); // inner again
    dst.extend_from_slice(&src[e2 + 2..]); // after :|
    dst
}

/// Parse ABC text and compile it to a time-sorted [`Tune`]. Returns `None`
/// only on gross failure (no events produced at all — e.g. empty input or
/// input with no notes); unknown constructs are skipped, never fatal
/// (`ParseTune`).
pub fn parse_tune(text: &str) -> Option<Tune> {
    let mut parser = Parser::new();
    let bytes = text.as_bytes();
    for line in bytes.split(|&b| b == b'\n') {
        parser.process_line(line);
    }
    let tune = parser.finish();
    if tune.events.is_empty() {
        None
    } else {
        Some(tune)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uppercase C at the default octave, default state (K: absent, so no
    /// key signature; unit length 1/8; 4/4; 120bpm) is MIDI 48.
    const MIDI_C4: u8 = 48;

    #[test]
    fn twinkle_twinkle_default_state() {
        let tune = parse_tune("C C G G A A G2 F F E E D D C2").expect("should parse");
        // 14 note tokens * 2 events (on+off) + 1 trailing ProgramChange = 29.
        assert_eq!(tune.events.len(), 29);
        assert_eq!(tune.bpm, 120);

        // events[0] is the trailing ProgramChange, not the first note: the
        // final sort orders same-timestamp events by priority (NoteOff <
        // ControlChange < ProgramChange < NoteOn), and the ProgramChange for
        // each voice is pushed at time 0 — so it sorts ahead of every note-on
        // that also starts at time 0 (see `Abc.mod`'s `Sort`/`Pri`).
        assert_eq!(tune.events[0].status, 0xC0);
        assert_eq!(tune.events[0].time_ms, 0);

        let first_note_on = tune
            .events
            .iter()
            .find(|e| e.status == 0x90)
            .expect("expected a note-on");
        assert_eq!(first_note_on.time_ms, 0);
        assert_eq!(first_note_on.d1, MIDI_C4);
    }

    #[test]
    fn key_signature_g_makes_f_sharp() {
        // K:G puts one sharp in the key (F#). Natural F (uppercase, default
        // octave) is MIDI 53 under this ABC dialect's C=48 convention (see
        // MIDI_C4 above) — sharped, that's 54.
        let tune = parse_tune("K:G\nF2").expect("should parse");
        let note_on = tune
            .events
            .iter()
            .find(|e| e.status == 0x90)
            .expect("expected a note-on");
        assert_eq!(note_on.d1, 54, "F should be sharped to F# (53+1) under K:G");
    }

    #[test]
    fn no_key_signature_f_is_natural() {
        let tune = parse_tune("F2").expect("should parse");
        let note_on = tune.events.iter().find(|e| e.status == 0x90).unwrap();
        assert_eq!(
            note_on.d1, 53,
            "F should be natural (F4=53 under this dialect's C=48) with no key sig"
        );
    }

    #[test]
    fn tied_notes_merge_into_one_longer_note() {
        // C2-C2 tied should merge into ONE note-on/note-off pair covering
        // the combined duration, not two separate notes.
        let tune = parse_tune("C2-C2").expect("should parse");
        let note_ons: Vec<_> = tune.events.iter().filter(|e| e.status == 0x90).collect();
        let note_offs: Vec<_> = tune.events.iter().filter(|e| e.status == 0x80).collect();
        assert_eq!(note_ons.len(), 1, "tied notes should merge to one note-on");
        assert_eq!(
            note_offs.len(),
            1,
            "tied notes should merge to one note-off"
        );
        // duration = 2 + 2 = 4 eighth-units = 4/8 whole = 0.5 whole notes;
        // msPerWhole = 4*60000/120 = 2000ms -> 1000ms duration.
        assert_eq!(note_ons[0].time_ms, 0);
        assert_eq!(note_offs[0].time_ms, 1000);
    }

    #[test]
    fn untied_notes_stay_separate() {
        let tune = parse_tune("C2 C2").expect("should parse");
        let note_ons: Vec<_> = tune.events.iter().filter(|e| e.status == 0x90).collect();
        assert_eq!(note_ons.len(), 2, "untied repeated notes stay separate");
    }

    #[test]
    fn chord_emits_three_simultaneous_note_ons() {
        let tune = parse_tune("[CEG]2").expect("should parse");
        let note_ons: Vec<_> = tune.events.iter().filter(|e| e.status == 0x90).collect();
        assert_eq!(note_ons.len(), 3, "[CEG] should emit 3 note-ons");
        // All three should start at the same time (t=0).
        for e in &note_ons {
            assert_eq!(e.time_ms, 0);
        }
        let pitches: std::collections::HashSet<u8> = note_ons.iter().map(|e| e.d1).collect();
        // C=48, E=52, G=55 (uppercase, default octave, no key sig).
        assert_eq!(pitches, std::collections::HashSet::from([48, 52, 55]));
    }

    #[test]
    fn repeat_expansion_plays_enclosed_material_twice() {
        let plain = parse_tune("C D E F").expect("should parse");
        let repeated = parse_tune("|: C D :| E F").expect("should parse");
        let plain_notes = plain.events.iter().filter(|e| e.status == 0x90).count();
        let repeated_notes = repeated.events.iter().filter(|e| e.status == 0x90).count();
        // "C D" repeated = 4 notes total, plus "E F" after = 6 total, vs
        // plain "C D E F" = 4. So repeated should have 2 more note-ons than
        // plain (the extra C D repeat).
        assert_eq!(plain_notes, 4);
        assert_eq!(repeated_notes, 6);
    }

    #[test]
    fn two_voices_get_distinct_midi_channels() {
        let text = "V:1\nC C\nV:2\nE E";
        let tune = parse_tune(text).expect("should parse");
        let note_ons: Vec<_> = tune.events.iter().filter(|e| e.status == 0x90).collect();
        assert_eq!(note_ons.len(), 4);
        let channels: std::collections::HashSet<u8> = note_ons.iter().map(|e| e.chan).collect();
        assert_eq!(
            channels.len(),
            2,
            "each voice should get its own MIDI channel"
        );
        assert!(channels.contains(&0), "voice 1 should be channel 0");
        assert!(channels.contains(&1), "voice 2 should be channel 1");
    }

    #[test]
    fn empty_input_still_parses_because_of_the_trailing_program_change() {
        // `Abc.def`'s own doc comment claims "FALSE only on gross failure
        // (empty / no notes)" — but tracing `ParseTune`'s actual body shows
        // the trailing `FOR i := 0 TO gNVoices-1 DO Push(0, ProgramChange,
        // ...) END` runs UNCONDITIONALLY (gNVoices starts at 1 and nothing
        // ever reduces it), so `gT^.count > 0` holds for every input,
        // including "". The doc comment doesn't match the code it's
        // documenting; this test follows the real, verified behavior, not
        // the comment's claim.
        let tune =
            parse_tune("").expect("even empty input produces the trailing ProgramChange event");
        assert_eq!(tune.events.len(), 1);
        assert_eq!(tune.events[0].status, 0xC0);
    }

    #[test]
    fn header_only_no_notes_still_parses_to_just_the_program_change() {
        let tune = parse_tune("X:1\nT:Empty\nK:C\n").expect("program-change event still counts");
        assert_eq!(tune.events.len(), 1);
        assert_eq!(tune.events[0].status, 0xC0);
    }

    #[test]
    fn rest_advances_cursor_without_emitting_notes() {
        let tune = parse_tune("C z C").expect("should parse");
        let note_ons: Vec<_> = tune.events.iter().filter(|e| e.status == 0x90).collect();
        assert_eq!(note_ons.len(), 2);
        assert!(note_ons[1].time_ms > note_ons[0].time_ms);
    }

    #[test]
    fn bar_line_resets_accidental() {
        // ^F sharpens F for the rest of the bar; after a bar line the
        // accidental should reset to natural.
        let tune = parse_tune("^F F | F").expect("should parse");
        let note_ons: Vec<_> = tune.events.iter().filter(|e| e.status == 0x90).collect();
        assert_eq!(note_ons.len(), 3);
        assert_eq!(note_ons[0].d1, 54); // explicit sharp F# (natural F=53, +1)
        assert_eq!(note_ons[1].d1, 54); // persists within the bar
        assert_eq!(note_ons[2].d1, 53); // resets to natural F after the bar line
    }

    #[test]
    fn lowercase_c_is_octave_above_uppercase() {
        let tune = parse_tune("c").expect("should parse");
        let note_on = tune.events.iter().find(|e| e.status == 0x90).unwrap();
        assert_eq!(note_on.d1, 60, "lowercase c should be MIDI 60");
    }

    #[test]
    fn tempo_header_changes_bpm() {
        let tune = parse_tune("Q:90\nK:C\nC").expect("should parse");
        assert_eq!(tune.bpm, 90);
    }
}
