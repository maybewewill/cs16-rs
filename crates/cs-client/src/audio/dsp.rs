//! GoldSrc 29-preset room reverb/echo DSP (`room_type 0..=28`) + underwater muffling.

pub const NUM_ROOM_TYPES: usize = 29;

#[derive(Debug, Clone, Copy)]
pub struct RoomPreset {
    pub room_lp: bool,
    pub room_mod_depth: f32,
    pub room_size: f32,
    pub room_refl: f32,
    pub room_rvb: f32,
    pub room_rvblp: bool,
    pub room_dly: f32,
    pub room_feedback: f32,
    pub room_dlylp: f32,
    pub room_left: f32,
}

/// GoldSrc `sx_presets` (Valve `room_type 0..=28`).
pub const ROOM_PRESETS: [RoomPreset; NUM_ROOM_TYPES] = [
    // 0: Normal / Off
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.0, room_refl: 0.0, room_rvb: 0.0, room_rvblp: false, room_dly: 0.0, room_feedback: 0.0, room_dlylp: 0.0, room_left: 0.0 },
    // 1: Generic
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.0, room_refl: 0.0, room_rvb: 0.0, room_rvblp: true, room_dly: 0.065, room_feedback: 0.1, room_dlylp: 1.0, room_left: 0.01 },
    // 2: Metal Small
    RoomPreset { room_lp: false, room_mod_depth: 0.1, room_size: 0.055, room_refl: 0.5, room_rvb: 0.6, room_rvblp: true, room_dly: 0.0, room_feedback: 0.0, room_dlylp: 0.0, room_left: 0.01 },
    // 3: Metal Medium
    RoomPreset { room_lp: false, room_mod_depth: 0.1, room_size: 0.1, room_refl: 0.5, room_rvb: 0.65, room_rvblp: true, room_dly: 0.0, room_feedback: 0.0, room_dlylp: 0.0, room_left: 0.02 },
    // 4: Metal Large
    RoomPreset { room_lp: false, room_mod_depth: 0.1, room_size: 0.18, room_refl: 0.5, room_rvb: 0.75, room_rvblp: true, room_dly: 0.0, room_feedback: 0.0, room_dlylp: 0.0, room_left: 0.03 },
    // 5: Tunnel Small
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.075, room_refl: 0.5, room_rvb: 0.55, room_rvblp: true, room_dly: 0.08, room_feedback: 0.25, room_dlylp: 1.0, room_left: 0.01 },
    // 6: Tunnel Medium
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.14, room_refl: 0.5, room_rvb: 0.65, room_rvblp: true, room_dly: 0.16, room_feedback: 0.35, room_dlylp: 1.0, room_left: 0.02 },
    // 7: Tunnel Large
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.24, room_refl: 0.5, room_rvb: 0.75, room_rvblp: true, room_dly: 0.28, room_feedback: 0.45, room_dlylp: 1.0, room_left: 0.03 },
    // 8: Chamber Small
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.065, room_refl: 0.5, room_rvb: 0.6, room_rvblp: true, room_dly: 0.1, room_feedback: 0.3, room_dlylp: 1.0, room_left: 0.01 },
    // 9: Chamber Medium
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.12, room_refl: 0.5, room_rvb: 0.7, room_rvblp: true, room_dly: 0.18, room_feedback: 0.4, room_dlylp: 1.0, room_left: 0.02 },
    // 10: Chamber Large
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.22, room_refl: 0.5, room_rvb: 0.78, room_rvblp: true, room_dly: 0.26, room_feedback: 0.48, room_dlylp: 1.0, room_left: 0.03 },
    // 11: Bright Small
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.055, room_refl: 0.55, room_rvb: 0.65, room_rvblp: false, room_dly: 0.0, room_feedback: 0.0, room_dlylp: 0.0, room_left: 0.01 },
    // 12: Bright Medium
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.12, room_refl: 0.55, room_rvb: 0.75, room_rvblp: false, room_dly: 0.0, room_feedback: 0.0, room_dlylp: 0.0, room_left: 0.02 },
    // 13: Bright Large
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.2, room_refl: 0.55, room_rvb: 0.85, room_rvblp: false, room_dly: 0.0, room_feedback: 0.0, room_dlylp: 0.0, room_left: 0.03 },
    // 14: Water 1
    RoomPreset { room_lp: true, room_mod_depth: 0.6, room_size: 0.06, room_refl: 0.5, room_rvb: 0.6, room_rvblp: true, room_dly: 0.0, room_feedback: 0.0, room_dlylp: 1.0, room_left: 0.02 },
    // 15: Water 2
    RoomPreset { room_lp: true, room_mod_depth: 0.75, room_size: 0.1, room_refl: 0.55, room_rvb: 0.75, room_rvblp: true, room_dly: 0.2, room_feedback: 0.3, room_dlylp: 1.0, room_left: 0.04 },
    // 16: Water 3
    RoomPreset { room_lp: true, room_mod_depth: 0.9, room_size: 0.14, room_refl: 0.6, room_rvb: 0.85, room_rvblp: true, room_dly: 0.35, room_feedback: 0.5, room_dlylp: 1.0, room_left: 0.06 },
    // 17: Concrete Small
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.07, room_refl: 0.5, room_rvb: 0.62, room_rvblp: true, room_dly: 0.11, room_feedback: 0.32, room_dlylp: 0.8, room_left: 0.01 },
    // 18: Concrete Medium
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.13, room_refl: 0.5, room_rvb: 0.72, room_rvblp: true, room_dly: 0.19, room_feedback: 0.42, room_dlylp: 0.8, room_left: 0.02 },
    // 19: Concrete Large
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.24, room_refl: 0.5, room_rvb: 0.8, room_rvblp: true, room_dly: 0.3, room_feedback: 0.5, room_dlylp: 0.8, room_left: 0.03 },
    // 20: Big 1
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.26, room_refl: 0.45, room_rvb: 0.65, room_rvblp: true, room_dly: 0.35, room_feedback: 0.42, room_dlylp: 1.0, room_left: 0.03 },
    // 21: Big 2
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.32, room_refl: 0.45, room_rvb: 0.75, room_rvblp: true, room_dly: 0.45, room_feedback: 0.5, room_dlylp: 1.0, room_left: 0.04 },
    // 22: Big 3
    RoomPreset { room_lp: false, room_mod_depth: 0.0, room_size: 0.4, room_refl: 0.45, room_rvb: 0.85, room_rvblp: true, room_dly: 0.55, room_feedback: 0.58, room_dlylp: 1.0, room_left: 0.05 },
    // 23: Cavern 1
    RoomPreset { room_lp: false, room_mod_depth: 0.2, room_size: 0.28, room_refl: 0.5, room_rvb: 0.72, room_rvblp: true, room_dly: 0.4, room_feedback: 0.5, room_dlylp: 1.0, room_left: 0.04 },
    // 24: Cavern 2
    RoomPreset { room_lp: false, room_mod_depth: 0.25, room_size: 0.36, room_refl: 0.5, room_rvb: 0.8, room_rvblp: true, room_dly: 0.5, room_feedback: 0.58, room_dlylp: 1.0, room_left: 0.05 },
    // 25: Cavern 3
    RoomPreset { room_lp: false, room_mod_depth: 0.3, room_size: 0.45, room_refl: 0.5, room_rvb: 0.88, room_rvblp: true, room_dly: 0.65, room_feedback: 0.65, room_dlylp: 1.0, room_left: 0.06 },
    // 26: Weirdo 1
    RoomPreset { room_lp: true, room_mod_depth: 0.8, room_size: 0.08, room_refl: 0.6, room_rvb: 0.75, room_rvblp: false, room_dly: 0.14, room_feedback: 0.6, room_dlylp: 0.0, room_left: 0.04 },
    // 27: Weirdo 2
    RoomPreset { room_lp: true, room_mod_depth: 0.9, room_size: 0.18, room_refl: 0.65, room_rvb: 0.82, room_rvblp: false, room_dly: 0.24, room_feedback: 0.7, room_dlylp: 0.0, room_left: 0.05 },
    // 28: Weirdo 3
    RoomPreset { room_lp: true, room_mod_depth: 1.0, room_size: 0.3, room_refl: 0.7, room_rvb: 0.9, room_rvblp: false, room_dly: 0.4, room_feedback: 0.8, room_dlylp: 0.0, room_left: 0.06 },
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
            let refl_l = self.early[0].read_at(left_d) * preset.room_refl;
            let refl_r = self.early[1].read_at(size_d) * preset.room_refl;
            self.early[0].write_advance(in_pair[0] + refl_r * 0.25);
            self.early[1].write_advance(in_pair[1] + refl_l * 0.25);

            let mut rvb_out = [0.0f32; 2];
            if preset.room_rvb > 0.0 {
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
                        let fb = (base_fb * preset.room_rvb).clamp(0.0, 0.88);
                        line.write_advance(src * 0.25 + filtered * fb);
                        sum += filtered;
                    }
                    rvb_out[ch] = sum * preset.room_rvb * 0.35;
                }
            }

            let mut dly_out = [0.0f32; 2];
            if preset.room_dly > 0.0 && preset.room_feedback > 0.0 {
                let dly_samples = (preset.room_dly * sr).round().max(8.0) as usize;
                for ch in 0..2 {
                    let tap = self.echo[ch].read_at(dly_samples + ch * 37);
                    let filt = if preset.room_dlylp > 0.0 {
                        self.echo[ch].lp_state +=
                            (tap - self.echo[ch].lp_state) * (1.0 - 0.55 * preset.room_dlylp.clamp(0.0, 1.0));
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
