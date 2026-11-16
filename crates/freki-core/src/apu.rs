//! The Audio Processing Unit: two square channels (one with sweep), a wave channel and
//! a noise channel, mixed to stereo ($FF10–$FF3F).
//!
//! The APU is clocked per T-cycle. When a sample rate is set with [`Apu::set_sample_rate`],
//! it point-samples the mixed output into an interleaved stereo `f32` buffer that the
//! frontend drains with [`Apu::drain_samples`].

use crate::CLOCK_HZ;

/// The frame sequencer runs at 512 Hz.
const FRAME_SEQUENCER_PERIOD: u32 = CLOCK_HZ / 512;

const DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1], // 12.5 %
    [1, 0, 0, 0, 0, 0, 0, 1], // 25 %
    [1, 0, 0, 0, 0, 1, 1, 1], // 50 %
    [0, 1, 1, 1, 1, 1, 1, 0], // 75 %
];

/// Bits that always read as 1 for $FF10–$FF2F (unused bits and write-only registers).
const READ_MASKS: [u8; 0x20] = [
    0x80, 0x3F, 0x00, 0xFF, 0xBF, // NR10–NR14
    0xFF, 0x3F, 0x00, 0xFF, 0xBF, // (unused), NR21–NR24
    0x7F, 0xFF, 0x9F, 0xFF, 0xBF, // NR30–NR34
    0xFF, 0xFF, 0x00, 0x00, 0xBF, // (unused), NR41–NR44
    0x00, 0x00, 0x70, // NR50, NR51, NR52
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, // $FF27–$FF2F
];

/// Length counter: silences a channel after a programmable time.
#[derive(Default)]
struct Length {
    counter: u16,
    enabled: bool,
}

impl Length {
    /// Clocked at 256 Hz. Returns `false` when the channel should be switched off.
    fn clock(&mut self) -> bool {
        if self.enabled && self.counter > 0 {
            self.counter -= 1;
            return self.counter > 0;
        }
        true
    }

    /// Handles a write to the length-enable and trigger bits of NRx4.
    ///
    /// `first_half` is true when the next frame-sequencer step does not clock length.
    /// In that case enabling length (or triggering with it enabled) clocks it once extra.
    /// Returns `false` if the extra clock expired the counter and the channel must stop.
    fn write_control(&mut self, enable: bool, trigger: bool, max: u16, first_half: bool) -> bool {
        let was_enabled = self.enabled;
        self.enabled = enable;
        let mut keep = true;
        if !was_enabled && enable && first_half && self.counter > 0 {
            self.counter -= 1;
            keep = self.counter > 0 || trigger;
        }
        if trigger && self.counter == 0 {
            self.counter = max;
            if enable && first_half {
                self.counter -= 1;
            }
        }
        keep
    }
}

/// Volume envelope (NRx2).
#[derive(Default)]
struct Envelope {
    initial: u8,
    increase: bool,
    pace: u8,
    volume: u8,
    timer: u8,
}

impl Envelope {
    fn write(&mut self, value: u8) {
        self.initial = value >> 4;
        self.increase = value & 0x08 != 0;
        self.pace = value & 0x07;
    }

    /// The DAC is on when the upper 5 bits of NRx2 are not all zero.
    fn dac_on(&self) -> bool {
        self.initial != 0 || self.increase
    }

    fn trigger(&mut self) {
        self.volume = self.initial;
        self.timer = self.pace;
    }

    /// Clocked at 64 Hz.
    fn clock(&mut self) {
        if self.pace == 0 {
            return;
        }
        self.timer = self.timer.saturating_sub(1);
        if self.timer == 0 {
            self.timer = self.pace;
            if self.increase && self.volume < 15 {
                self.volume += 1;
            } else if !self.increase && self.volume > 0 {
                self.volume -= 1;
            }
        }
    }
}

/// Channels 1 and 2. Only channel 1 uses the frequency sweep.
#[derive(Default)]
struct Square {
    on: bool,
    duty: u8,
    duty_step: u8,
    period: u16,
    timer: u32,
    length: Length,
    envelope: Envelope,

    sweep_pace: u8,
    sweep_decrease: bool,
    sweep_step: u8,
    sweep_timer: u8,
    sweep_enabled: bool,
    shadow_period: u16,
    /// A sweep calculation in decrease mode happened since the last trigger.
    sweep_negated: bool,
}

impl Square {
    fn tick(&mut self) {
        if self.timer > 0 {
            self.timer -= 1;
        }
        if self.timer == 0 {
            self.timer = (2048 - self.period as u32) * 4;
            self.duty_step = (self.duty_step + 1) & 7;
        }
    }

    fn output(&self) -> u8 {
        if self.on {
            DUTY_TABLE[self.duty as usize][self.duty_step as usize] * self.envelope.volume
        } else {
            0
        }
    }

    fn trigger(&mut self) {
        self.on = self.envelope.dac_on();
        self.timer = (2048 - self.period as u32) * 4;
        self.envelope.trigger();

        self.shadow_period = self.period;
        self.sweep_negated = false;
        self.sweep_timer = if self.sweep_pace == 0 {
            8
        } else {
            self.sweep_pace
        };
        self.sweep_enabled = self.sweep_pace != 0 || self.sweep_step != 0;
        if self.sweep_step != 0 {
            self.sweep_calculate();
        }
    }

    /// Computes the next sweep period; switches the channel off if it overflows.
    fn sweep_calculate(&mut self) -> u16 {
        let delta = self.shadow_period >> self.sweep_step;
        let next = if self.sweep_decrease {
            self.sweep_negated = true;
            self.shadow_period.wrapping_sub(delta)
        } else {
            self.shadow_period + delta
        };
        if next > 2047 {
            self.on = false;
        }
        next
    }

    /// Clocked at 128 Hz.
    fn clock_sweep(&mut self) {
        self.sweep_timer = self.sweep_timer.saturating_sub(1);
        if self.sweep_timer != 0 {
            return;
        }
        self.sweep_timer = if self.sweep_pace == 0 {
            8
        } else {
            self.sweep_pace
        };
        if self.sweep_enabled && self.sweep_pace != 0 {
            let next = self.sweep_calculate();
            if next <= 2047 && self.sweep_step != 0 {
                self.period = next;
                self.shadow_period = next;
                self.sweep_calculate();
            }
        }
    }
}

/// Channel 3: plays 32 4-bit samples from wave RAM.
#[derive(Default)]
struct Wave {
    on: bool,
    dac_on: bool,
    /// NR32 output level: 0 = mute, 1 = 100 %, 2 = 50 %, 3 = 25 %.
    level: u8,
    period: u16,
    timer: u32,
    position: u8,
    ram: [u8; 16],
    length: Length,
}

impl Wave {
    fn tick(&mut self) {
        if self.timer > 0 {
            self.timer -= 1;
        }
        if self.timer == 0 {
            self.timer = (2048 - self.period as u32) * 2;
            self.position = (self.position + 1) & 31;
        }
    }

    fn output(&self) -> u8 {
        if !self.on || self.level == 0 {
            return 0;
        }
        let byte = self.ram[self.position as usize / 2];
        let sample = if self.position % 2 == 0 {
            byte >> 4
        } else {
            byte & 0x0F
        };
        sample >> (self.level - 1)
    }

    fn trigger(&mut self) {
        self.on = self.dac_on;
        self.timer = (2048 - self.period as u32) * 2;
        self.position = 0;
    }
}

/// Channel 4: pseudo-random noise from a linear-feedback shift register.
#[derive(Default)]
struct Noise {
    on: bool,
    shift: u8,
    short_mode: bool,
    divisor_code: u8,
    timer: u32,
    lfsr: u16,
    length: Length,
    envelope: Envelope,
}

impl Noise {
    fn timer_period(&self) -> u32 {
        let divisor = if self.divisor_code == 0 {
            8
        } else {
            self.divisor_code as u32 * 16
        };
        divisor << self.shift
    }

    fn tick(&mut self) {
        if self.timer > 0 {
            self.timer -= 1;
        }
        if self.timer == 0 {
            self.timer = self.timer_period();
            let bit = (self.lfsr ^ (self.lfsr >> 1)) & 1;
            self.lfsr = (self.lfsr >> 1) | (bit << 14);
            if self.short_mode {
                self.lfsr = (self.lfsr & !(1 << 6)) | (bit << 6);
            }
        }
    }

    fn output(&self) -> u8 {
        if self.on && self.lfsr & 1 == 0 {
            self.envelope.volume
        } else {
            0
        }
    }

    fn trigger(&mut self) {
        self.on = self.envelope.dac_on();
        self.timer = self.timer_period();
        self.envelope.trigger();
        self.lfsr = 0x7FFF;
    }
}

pub struct Apu {
    powered: bool,
    /// Raw register values for $FF10–$FF2F, used for reads.
    regs: [u8; 0x20],
    ch1: Square,
    ch2: Square,
    ch3: Wave,
    ch4: Noise,
    frame_timer: u32,
    frame_step: u8,

    /// Output sample rate in Hz; `None` means no samples are produced.
    sample_rate: Option<u32>,
    /// Fixed-point sample clock: adds `sample_rate` per T-cycle, emits at `CLOCK_HZ`.
    sample_clock: u32,
    samples: Vec<f32>,
    /// High-pass filter state (removes the DC offset, like the real hardware's capacitor).
    capacitor: [f32; 2],
    charge_factor: f32,
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

impl Apu {
    pub fn new() -> Self {
        let mut apu = Self {
            powered: true,
            regs: [0; 0x20],
            ch1: Square::default(),
            ch2: Square::default(),
            ch3: Wave::default(),
            ch4: Noise::default(),
            frame_timer: FRAME_SEQUENCER_PERIOD,
            frame_step: 0,
            sample_rate: None,
            sample_clock: 0,
            samples: Vec::new(),
            capacitor: [0.0; 2],
            charge_factor: 1.0,
        };
        // Register state after the DMG boot ROM.
        for (addr, value) in [
            (0xFF10, 0x80),
            (0xFF11, 0xBF),
            (0xFF12, 0xF3),
            (0xFF14, 0xBF),
            (0xFF16, 0x3F),
            (0xFF19, 0xBF),
            (0xFF1A, 0x7F),
            (0xFF1B, 0xFF),
            (0xFF1C, 0x9F),
            (0xFF1E, 0xBF),
            (0xFF20, 0xFF),
            (0xFF23, 0xBF),
            (0xFF24, 0x77),
            (0xFF25, 0xF3),
        ] {
            apu.write(addr, value);
        }
        // The boot "ding" has faded by now: channel 1 still reports as on, but silent.
        apu.ch1.envelope.volume = 0;
        apu
    }

    /// Starts producing interleaved stereo samples at `rate` Hz.
    pub fn set_sample_rate(&mut self, rate: u32) {
        self.sample_rate = Some(rate);
        self.sample_clock = 0;
        // The DMG's output capacitor discharges by 0.999958 per T-cycle.
        self.charge_factor = 0.999_958_f32.powf(CLOCK_HZ as f32 / rate as f32);
    }

    /// Moves all samples produced so far into `out` (interleaved left, right).
    pub fn drain_samples(&mut self, out: &mut Vec<f32>) {
        out.append(&mut self.samples);
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF26 => {
                let status = u8::from(self.ch1.on)
                    | u8::from(self.ch2.on) << 1
                    | u8::from(self.ch3.on) << 2
                    | u8::from(self.ch4.on) << 3;
                0x70 | u8::from(self.powered) << 7 | status
            }
            0xFF10..=0xFF2F => {
                let i = (addr - 0xFF10) as usize;
                self.regs[i] | READ_MASKS[i]
            }
            0xFF30..=0xFF3F => self.ch3.ram[(addr - 0xFF30) as usize],
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        if let 0xFF30..=0xFF3F = addr {
            self.ch3.ram[(addr - 0xFF30) as usize] = value;
            return;
        }
        if addr == 0xFF26 {
            let power = value & 0x80 != 0;
            if self.powered && !power {
                // Powering off clears every register. Wave RAM and, on the DMG, the
                // length counters survive.
                let ram = self.ch3.ram;
                let lengths = [
                    self.ch1.length.counter,
                    self.ch2.length.counter,
                    self.ch3.length.counter,
                    self.ch4.length.counter,
                ];
                *self = Self {
                    sample_rate: self.sample_rate,
                    sample_clock: self.sample_clock,
                    samples: std::mem::take(&mut self.samples),
                    capacitor: self.capacitor,
                    charge_factor: self.charge_factor,
                    ..Self::powered_off()
                };
                self.ch3.ram = ram;
                self.ch1.length.counter = lengths[0];
                self.ch2.length.counter = lengths[1];
                self.ch3.length.counter = lengths[2];
                self.ch4.length.counter = lengths[3];
            } else if !self.powered && power {
                self.frame_step = 0;
                self.frame_timer = FRAME_SEQUENCER_PERIOD;
            }
            self.powered = power;
            return;
        }
        if !(0xFF10..=0xFF25).contains(&addr) {
            return;
        }
        if !self.powered {
            // On the DMG, length counters can still be loaded while the APU is off.
            match addr {
                0xFF11 => self.ch1.length.counter = 64 - (value & 0x3F) as u16,
                0xFF16 => self.ch2.length.counter = 64 - (value & 0x3F) as u16,
                0xFF1B => self.ch3.length.counter = 256 - value as u16,
                0xFF20 => self.ch4.length.counter = 64 - (value & 0x3F) as u16,
                _ => {}
            }
            return;
        }
        // Next frame-sequencer step doesn't clock length (see `Length::write_control`).
        let first_half = self.frame_step % 2 == 1;
        let enable = value & 0x40 != 0;
        let trigger = value & 0x80 != 0;
        self.regs[(addr - 0xFF10) as usize] = value;

        match addr {
            // Channel 1
            0xFF10 => {
                self.ch1.sweep_pace = (value >> 4) & 0x07;
                self.ch1.sweep_decrease = value & 0x08 != 0;
                self.ch1.sweep_step = value & 0x07;
                if self.ch1.sweep_negated && !self.ch1.sweep_decrease {
                    self.ch1.on = false;
                }
            }
            0xFF11 => {
                self.ch1.duty = value >> 6;
                self.ch1.length.counter = 64 - (value & 0x3F) as u16;
            }
            0xFF12 => {
                self.ch1.envelope.write(value);
                self.ch1.on &= self.ch1.envelope.dac_on();
            }
            0xFF13 => self.ch1.period = (self.ch1.period & 0x700) | value as u16,
            0xFF14 => {
                self.ch1.period = (self.ch1.period & 0xFF) | ((value as u16 & 0x07) << 8);
                let keep = self
                    .ch1
                    .length
                    .write_control(enable, trigger, 64, first_half);
                if trigger {
                    self.ch1.trigger();
                }
                self.ch1.on &= keep;
            }
            // Channel 2
            0xFF16 => {
                self.ch2.duty = value >> 6;
                self.ch2.length.counter = 64 - (value & 0x3F) as u16;
            }
            0xFF17 => {
                self.ch2.envelope.write(value);
                self.ch2.on &= self.ch2.envelope.dac_on();
            }
            0xFF18 => self.ch2.period = (self.ch2.period & 0x700) | value as u16,
            0xFF19 => {
                self.ch2.period = (self.ch2.period & 0xFF) | ((value as u16 & 0x07) << 8);
                let keep = self
                    .ch2
                    .length
                    .write_control(enable, trigger, 64, first_half);
                if trigger {
                    self.ch2.trigger();
                }
                self.ch2.on &= keep;
            }
            // Channel 3
            0xFF1A => {
                self.ch3.dac_on = value & 0x80 != 0;
                self.ch3.on &= self.ch3.dac_on;
            }
            0xFF1B => self.ch3.length.counter = 256 - value as u16,
            0xFF1C => self.ch3.level = (value >> 5) & 0x03,
            0xFF1D => self.ch3.period = (self.ch3.period & 0x700) | value as u16,
            0xFF1E => {
                self.ch3.period = (self.ch3.period & 0xFF) | ((value as u16 & 0x07) << 8);
                let keep = self
                    .ch3
                    .length
                    .write_control(enable, trigger, 256, first_half);
                if trigger {
                    self.ch3.trigger();
                }
                self.ch3.on &= keep;
            }
            // Channel 4
            0xFF20 => self.ch4.length.counter = 64 - (value & 0x3F) as u16,
            0xFF21 => {
                self.ch4.envelope.write(value);
                self.ch4.on &= self.ch4.envelope.dac_on();
            }
            0xFF22 => {
                self.ch4.shift = value >> 4;
                self.ch4.short_mode = value & 0x08 != 0;
                self.ch4.divisor_code = value & 0x07;
            }
            0xFF23 => {
                let keep = self
                    .ch4
                    .length
                    .write_control(enable, trigger, 64, first_half);
                if trigger {
                    self.ch4.trigger();
                }
                self.ch4.on &= keep;
            }
            _ => {} // NR50, NR51: read back from `regs` when mixing
        }
    }

    fn powered_off() -> Self {
        Self {
            powered: false,
            regs: [0; 0x20],
            ch1: Square::default(),
            ch2: Square::default(),
            ch3: Wave::default(),
            ch4: Noise::default(),
            frame_timer: FRAME_SEQUENCER_PERIOD,
            frame_step: 0,
            sample_rate: None,
            sample_clock: 0,
            samples: Vec::new(),
            capacitor: [0.0; 2],
            charge_factor: 1.0,
        }
    }

    /// Advances the APU by `cycles` T-cycles.
    pub fn tick(&mut self, cycles: u32) {
        for _ in 0..cycles {
            if self.powered {
                self.tick_cycle();
            }
            if let Some(rate) = self.sample_rate {
                self.sample_clock += rate;
                if self.sample_clock >= CLOCK_HZ {
                    self.sample_clock -= CLOCK_HZ;
                    self.push_sample();
                }
            }
        }
    }

    fn tick_cycle(&mut self) {
        self.ch1.tick();
        self.ch2.tick();
        self.ch3.tick();
        self.ch4.tick();

        self.frame_timer -= 1;
        if self.frame_timer == 0 {
            self.frame_timer = FRAME_SEQUENCER_PERIOD;
            self.clock_frame_sequencer();
        }
    }

    /// 512 Hz: length at steps 0/2/4/6, sweep at 2/6, envelope at 7.
    fn clock_frame_sequencer(&mut self) {
        let step = self.frame_step;
        if step % 2 == 0 {
            self.ch1.on &= self.ch1.length.clock();
            self.ch2.on &= self.ch2.length.clock();
            self.ch3.on &= self.ch3.length.clock();
            self.ch4.on &= self.ch4.length.clock();
        }
        if step == 2 || step == 6 {
            self.ch1.clock_sweep();
        }
        if step == 7 {
            self.ch1.envelope.clock();
            self.ch2.envelope.clock();
            self.ch4.envelope.clock();
        }
        self.frame_step = (step + 1) & 7;
    }

    /// Converts a channel's 0–15 output to −1.0…1.0, or 0 if its DAC is off.
    fn dac(on: bool, digital: u8) -> f32 {
        if on { digital as f32 / 7.5 - 1.0 } else { 0.0 }
    }

    fn push_sample(&mut self) {
        let channels = [
            Self::dac(self.ch1.envelope.dac_on(), self.ch1.output()),
            Self::dac(self.ch2.envelope.dac_on(), self.ch2.output()),
            Self::dac(self.ch3.dac_on, self.ch3.output()),
            Self::dac(self.ch4.envelope.dac_on(), self.ch4.output()),
        ];
        let nr50 = self.regs[0x14];
        let nr51 = self.regs[0x15];
        for side in 0..2 {
            // NR51: bits 0–3 route channels to the right, bits 4–7 to the left.
            let shift = if side == 0 { 4 } else { 0 };
            let mut mix: f32 = channels
                .iter()
                .enumerate()
                .filter(|&(i, _)| nr51 & (1 << (i + shift)) != 0)
                .map(|(_, &v)| v)
                .sum();
            // NR50: master volume 0–7 per side (left in bits 4–6, right in bits 0–2).
            let volume = ((nr50 >> shift) & 0x07) as f32 + 1.0;
            mix *= volume / 8.0 / 4.0;

            let out = mix - self.capacitor[side];
            self.capacitor[side] = mix - out * self.charge_factor;
            self.samples.push(if self.powered { out } else { 0.0 });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nr52_reports_triggered_channel_and_length_expiry() {
        let mut apu = Apu::new();
        apu.write(0xFF17, 0xF0); // ch2 volume 15, DAC on
        apu.write(0xFF16, 0x3F); // length 1
        apu.write(0xFF19, 0xC0); // trigger, length enabled
        assert_eq!(apu.read(0xFF26) & 0x02, 0x02);
        apu.tick(FRAME_SEQUENCER_PERIOD * 2);
        assert_eq!(apu.read(0xFF26) & 0x02, 0, "length expired");
    }

    #[test]
    fn power_off_clears_registers_but_keeps_wave_ram() {
        let mut apu = Apu::new();
        apu.write(0xFF30, 0xAB);
        apu.write(0xFF26, 0x00);
        assert_eq!(apu.read(0xFF24), 0x00);
        assert_eq!(apu.read(0xFF30), 0xAB);
        apu.write(0xFF24, 0x77); // ignored while off
        assert_eq!(apu.read(0xFF24), 0x00);
    }

    #[test]
    fn register_reads_use_masks() {
        let apu = Apu::new();
        assert_eq!(apu.read(0xFF13), 0xFF, "period low is write-only");
        assert_eq!(apu.read(0xFF26), 0xF1, "on, ch1 playing after boot");
    }

    #[test]
    fn produces_samples_at_requested_rate() {
        let mut apu = Apu::new();
        apu.set_sample_rate(48_000);
        apu.tick(CLOCK_HZ / 4); // exactly 0.25 s
        let mut out = Vec::new();
        apu.drain_samples(&mut out);
        assert_eq!(out.len(), 2 * 12_000);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    }

    #[test]
    fn square_wave_is_audible() {
        let mut apu = Apu::new();
        apu.set_sample_rate(48_000);
        apu.write(0xFF11, 0x80); // 50 % duty
        apu.write(0xFF12, 0xF0); // volume 15
        apu.write(0xFF13, 0x00);
        apu.write(0xFF14, 0x87); // trigger, period $700 (~440 Hz-ish)
        apu.tick(CLOCK_HZ / 20);
        let mut out = Vec::new();
        apu.drain_samples(&mut out);
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.05, "peak {peak}");
    }
}
