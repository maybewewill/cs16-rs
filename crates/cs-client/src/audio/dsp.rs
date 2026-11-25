//! GoldSrc 29-preset room reverb/echo DSP (`room_type 0..=28`) + underwater muffling.

pub const NUM_ROOM_TYPES: usize = 29;

#[derive(Debug, Clone, Copy)]
pub struct RoomPreset {
    pub room_lp: bool,
    pub room_mod_depth: f32,
    pub room_size: f32,
    pub room_refl: f32,
    pub room_rvblp: bool,
    pub room_dly: f32,
    pub room_feedback: f32,
    pub room_dlylp: f32,
    pub room_left: f32,
}

const fn preset(row: [f32; 9]) -> RoomPreset {
    RoomPreset {
        room_lp: row[0] != 0.0,
        room_mod_depth: row[1],
        room_size: row[2],
        room_refl: row[3],
        room_rvblp: row[4] != 0.0,
        room_dly: row[5],
        room_feedback: row[6],
        room_dlylp: row[7],
        room_left: row[8],
    }
}

/// Retail GoldSrc `sx_presets` 29x9 table (`hw.dll:0x1031c6c0..0x1031cad4`, `room_type 0..=28`).
pub const ROOM_PRESETS: [RoomPreset; NUM_ROOM_TYPES] = [
    preset([0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.0]),         // 0
    preset([0.0, 0.0, 0.0, 0.0, 1.0, 0.065, 0.1, 0.0, 0.01]),      // 1
    preset([0.0, 0.0, 0.0, 0.0, 1.0, 0.02, 0.75, 0.0, 0.01]),      // 2
    preset([0.0, 0.0, 0.0, 0.0, 1.0, 0.03, 0.78, 0.0, 0.02]),      // 3
    preset([0.0, 0.0, 0.0, 0.0, 1.0, 0.06, 0.77, 0.0, 0.03]),      // 4
    preset([0.0, 0.0, 0.05, 0.85, 1.0, 0.008, 0.96, 2.0, 0.01]),   // 5
    preset([0.0, 0.0, 0.05, 0.88, 1.0, 0.01, 0.98, 2.0, 0.02]),    // 6
    preset([0.0, 0.0, 0.05, 0.92, 1.0, 0.015, 0.995, 2.0, 0.04]),  // 7
    preset([0.0, 0.0, 0.05, 0.84, 1.0, 0.0, 0.0, 2.0, 0.012]),     // 8
    preset([0.0, 0.0, 0.05, 0.9, 1.0, 0.0, 0.0, 2.0, 0.008]),      // 9
    preset([0.0, 0.0, 0.05, 0.95, 1.0, 0.0, 0.0, 2.0, 0.004]),     // 10
    preset([0.0, 0.0, 0.05, 0.7, 0.0, 0.0, 0.0, 2.0, 0.012]),      // 11
    preset([0.0, 0.0, 0.055, 0.78, 0.0, 0.0, 0.0, 2.0, 0.008]),    // 12
    preset([0.0, 0.0, 0.05, 0.86, 0.0, 0.0, 0.0, 2.0, 0.002]),     // 13
    preset([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.01]),        // 14
    preset([1.0, 0.0, 0.0, 0.0, 1.0, 0.06, 0.85, 2.0, 0.02]),      // 15
    preset([1.0, 0.0, 0.0, 0.0, 1.0, 0.2, 0.6, 2.0, 0.05]),        // 16
    preset([0.0, 0.0, 0.05, 0.8, 1.0, 0.0, 0.48, 2.0, 0.016]),     // 17
    preset([0.0, 0.0, 0.06, 0.9, 1.0, 0.0, 0.52, 2.0, 0.01]),      // 18
    preset([0.0, 0.0, 0.07, 0.94, 1.0, 0.3, 0.6, 2.0, 0.008]),     // 19
    preset([0.0, 0.0, 0.0, 0.0, 1.0, 0.3, 0.42, 2.0, 0.0]),        // 20
    preset([0.0, 0.0, 0.0, 0.0, 1.0, 0.35, 0.48, 2.0, 0.0]),       // 21
    preset([0.0, 0.0, 0.0, 0.0, 1.0, 0.38, 0.6, 2.0, 0.0]),        // 22
    preset([0.0, 0.0, 0.05, 0.9, 1.0, 0.2, 0.28, 0.0, 0.0]),       // 23
    preset([0.0, 0.0, 0.07, 0.9, 1.0, 0.3, 0.4, 0.0, 0.0]),        // 24
    preset([0.0, 0.0, 0.09, 0.9, 1.0, 0.35, 0.5, 0.0, 0.0]),       // 25
    preset([0.0, 1.0, 0.01, 0.9, 0.0, 0.0, 0.0, 2.0, 0.05]),       // 26
    preset([0.0, 0.0, 0.0, 0.0, 1.0, 0.009, 0.999, 2.0, 0.04]),    // 27
    preset([0.0, 0.0, 0.001, 0.999, 0.0, 0.2, 0.8, 2.0, 0.05]),    // 28
];

#[derive(Debug, Clone)]
struct DelayLine {
    buf: Vec<f32>,
    idx: usize,
    lp_state: f32,
}

impl DelayLine {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(8)],
            idx: 0,
            lp_state: 0.0,
        }
    }

    fn read_at(&self, delay_samples: usize) -> f32 {
        let len = self.buf.len();
        let d = delay_samples.clamp(1, len - 1);
        let pos = (self.idx + len - d) % len;
        self.buf[pos]
    }

    fn write_advance(&mut self, val: f32) {
        self.buf[self.idx] = val;
        self.idx = (self.idx + 1) % self.buf.len();
    }
}

#[derive(Debug, Clone)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: [f32; 2],
    z2: [f32; 2],
}

impl Biquad {
    fn lowpass(sample_rate: f32, cutoff_hz: f32, q: f32) -> Self {
        let omega = 2.0 * std::f32::consts::PI * (cutoff_hz / sample_rate).clamp(0.001, 0.45);
        let sn = omega.sin();
        let cs = omega.cos();
        let alpha = sn / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - cs) * 0.5) / a0,
            b1: (1.0 - cs) / a0,
            b2: ((1.0 - cs) * 0.5) / a0,
            a1: (-2.0 * cs) / a0,
            a2: (1.0 - alpha) / a0,
            z1: [0.0; 2],
            z2: [0.0; 2],
        }
    }

    fn process(&mut self, ch: usize, x: f32) -> f32 {
        let y = self.b0 * x + self.z1[ch];
        self.z1[ch] = self.b1 * x - self.a1 * y + self.z2[ch];
        self.z2[ch] = self.b2 * x - self.a2 * y;
        y
    }
}

#[derive(Debug, Clone)]
pub struct RoomDsp {
    sample_rate: u32,
    room_type: usize,
    room_off: bool,
    underwater: bool,
    wet_ramp: f32,
    lfo_phase: f32,
    input_lp: [f32; 2],
    early: [DelayLine; 2],
    combs: [[DelayLine; 4]; 2],
    echo: [DelayLine; 2],
    underwater_lp: Biquad,
}

impl RoomDsp {
    pub fn new(sample_rate: u32) -> Self {
        let scale = (sample_rate as f32 / 44_100.0).max(0.25);
        let s = |n: usize| ((n as f32) * scale).round().max(16.0) as usize;
        Self {
            sample_rate,
            room_type: 0,
            room_off: false,
            underwater: false,
            wet_ramp: 0.0,
            lfo_phase: 0.0,
            input_lp: [0.0; 2],
            early: [DelayLine::new(s(22_050)), DelayLine::new(s(22_050))],
            combs: [
                [
                    DelayLine::new(s(1557)),
                    DelayLine::new(s(1617)),
                    DelayLine::new(s(1491)),
                    DelayLine::new(s(1422)),
                ],
                [
                    DelayLine::new(s(1603)),
                    DelayLine::new(s(1663)),
                    DelayLine::new(s(1537)),
                    DelayLine::new(s(1468)),
                ],
            ],
            echo: [DelayLine::new(s(44_100)), DelayLine::new(s(44_100))],
            underwater_lp: Biquad::lowpass(sample_rate as f32, 550.0, 0.707),
        }
    }

    pub fn set_room_type(&mut self, room_type: u16) {
        let idx = usize::from(room_type);
        self.room_type = if idx < NUM_ROOM_TYPES { idx } else { 0 };
    }

    pub fn room_type(&self) -> usize {
        self.room_type
    }

    pub fn set_room_off(&mut self, off: bool) {
        self.room_off = off;
    }

    pub fn set_underwater(&mut self, underwater: bool) {
        self.underwater = underwater;
    }

    pub fn process_frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        let active_room = if self.room_off { 0 } else { self.room_type };
        let preset = ROOM_PRESETS[active_room];
        let target_wet = if active_room == 0 { 0.0 } else { 1.0 };
        self.wet_ramp += (target_wet - self.wet_ramp) * 0.01;

        let (mut l, mut r) = (left, right);
        if self.wet_ramp > 1e-4 {
            let sr = self.sample_rate as f32;
            self.lfo_phase = (self.lfo_phase + 2.5 / sr) % 1.0;
            let mod_off = if preset.room_mod_depth > 0.0 {
                (self.lfo_phase * std::f32::consts::TAU).sin() * preset.room_mod_depth * 18.0
            } else {
                0.0
            };

            let mut in_pair = [l, r];
            if preset.room_lp {
                self.input_lp[0] += (in_pair[0] - self.input_lp[0]) * 0.25;
                self.input_lp[1] += (in_pair[1] - self.input_lp[1]) * 0.25;
                in_pair = self.input_lp;
            }

            let left_d = ((preset.room_left * sr + mod_off).round().max(1.0)) as usize;
            let size_d = ((preset.room_size * sr + mod_off).round().max(4.0)) as usize;
            let refl_l_gain = if preset.room_refl > 0.0 {
                preset.room_refl
            } else if preset.room_left > 0.0 {
                0.35
            } else {
                0.0
            };
            let refl_l = self.early[0].read_at(left_d) * refl_l_gain;
            let refl_r = self.early[1].read_at(size_d) * preset.room_refl;
            self.early[0].write_advance(in_pair[0] + refl_r * 0.25);
            self.early[1].write_advance(in_pair[1] + refl_l * 0.25);

            let mut rvb_out = [0.0f32; 2];
            if preset.room_size > 0.0 && preset.room_refl > 0.0 {
                let comb_delays = [
                    ((size_d / 2).max(8), 0.72),
                    (((size_d * 3) / 4).max(12), 0.68),
                    ((size_d).max(16), 0.64),
                    (((size_d * 5) / 4).max(20), 0.60),
                ];
                for ch in 0..2 {
                    let src = in_pair[ch] + if ch == 0 { refl_l } else { refl_r } * 0.35;
                    let mut sum = 0.0f32;
                    for (ci, (d, base_fb)) in comb_delays.iter().enumerate() {
                        let line = &mut self.combs[ch][ci];
                        let tap = line.read_at(*d + ch * 13);
                        let filtered = if preset.room_rvblp {
                            line.lp_state += (tap - line.lp_state) * 0.42;
                            line.lp_state
                        } else {
                            tap
                        };
                        let fb = (base_fb * preset.room_refl).clamp(0.0, 0.88);
                        line.write_advance(src * 0.25 + filtered * fb);
                        sum += filtered;
                    }
                    rvb_out[ch] = sum * preset.room_refl * 0.35;
                }
            }

            let mut dly_out = [0.0f32; 2];
            if preset.room_dly > 0.0 && preset.room_feedback > 0.0 {
                let dly_samples = (preset.room_dly * sr).round().max(8.0) as usize;
                let dlylp_norm = (preset.room_dlylp * 0.5).clamp(0.0, 1.0);
                for ch in 0..2 {
                    let tap = self.echo[ch].read_at(dly_samples + ch * 37);
                    let filt = if preset.room_dlylp > 0.0 {
                        self.echo[ch].lp_state +=
                            (tap - self.echo[ch].lp_state) * (1.0 - 0.55 * dlylp_norm);
                        self.echo[ch].lp_state
                    } else {
                        tap
                    };
                    let fb = preset.room_feedback.clamp(0.0, 0.85);
                    self.echo[ch].write_advance((in_pair[ch] + rvb_out[ch]) * 0.35 + filt * fb);
                    dly_out[ch] = filt;
                }
            }

            let wet_l = (refl_l + rvb_out[0] + dly_out[0]) * 0.55;
            let wet_r = (refl_r + rvb_out[1] + dly_out[1]) * 0.55;
            l += wet_l * self.wet_ramp;
            r += wet_r * self.wet_ramp;
        }

        if self.underwater {
            l = self.underwater_lp.process(0, l) * 0.85;
            r = self.underwater_lp.process(1, r) * 0.85;
        }

        (l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_29_presets_generate_tail_and_underwater_muffles_highs() {
        for rt in 1..NUM_ROOM_TYPES {
            let mut dsp = RoomDsp::new(44_100);
            dsp.set_room_type(rt as u16);
            let mut tail_energy = 0.0f32;
            for i in 0..22_050 {
                let inp = if i == 0 { 1.0 } else { 0.0 };
                let (l, r) = dsp.process_frame(inp, inp);
                if i > 100 {
                    tail_energy += l.abs() + r.abs();
                }
            }
            assert!(tail_energy > 0.01, "room_type {rt} should produce reverb/echo tail");
        }

        // Underwater filter attenuates 4 kHz tone much more strongly than 200 Hz tone.
        let measure = |freq_hz: f32| -> f32 {
            let mut dsp = RoomDsp::new(44_100);
            dsp.set_underwater(true);
            let mut energy = 0.0f32;
            for i in 0..4410 {
                let s = (2.0 * std::f32::consts::PI * freq_hz * (i as f32) / 44_100.0).sin();
                let (l, _) = dsp.process_frame(s, s);
                if i > 1000 {
                    energy += l * l;
                }
            }
            energy
        };
        let low = measure(200.0);
        let high = measure(4000.0);
        assert!(low > high * 20.0, "low={low} must vastly exceed high={high}");
    }
}
