use bitflags::bitflags;
use crossbeam::queue::ArrayQueue;

use super::dbg;
use super::IoReg;
use super::{InterruptSource, IrqSource};
use super::{MemR, MemW};

use std::sync::Arc;

const CLK_64_RELOAD: u32 = 4_194_304 / 64;
const CLK_128_RELOAD: u32 = 4_194_304 / 128;
const CLK_256_RELOAD: u32 = 4_194_304 / 256;

bitflags! {
    // NRx0 - Channel x Sweep register (R/W)
    struct NRx0: u8 {
        const SWEEP_TIME  = 0b_0111_0000;
        const SWEEP_NEG   = 0b_0000_1000;
        const SWEEP_SHIFT = 0b_0000_0111;
    }
}

bitflags! {
    // NRx1 - Channel x Sound Length/Wave Pattern Duty (R/W)
    struct NRx1: u8 {
        const WAVE_DUTY = 0b_1100_0000;
        const SOUND_LEN = 0b_0011_1111;
    }
}

bitflags! {
    // NRx2 - Channel x Volume Envelope (R/W)
    struct NRx2: u8 {
        const START_VOL  = 0b_1111_0000;
        const ENV_DIR    = 0b_0000_1000;
        const ENV_PERIOD = 0b_0000_0111;

        const DAC_ON     = 0b_1111_1000;
    }
}

bitflags! {
    // NRx4 - Channel x Frequency hi data (R/W)
    struct NRx4: u8 {
        const TRIGGER = 0b_1000_0000;
        const LEN_EN  = 0b_0100_0000;
        const FREQ_HI = 0b_0000_0111;
    }
}

bitflags! {
    // NR50 - Channel control / ON-OFF / Volume (R/W)
    struct NR50: u8 {
        const VIN_L_EN  = 0b_1000_0000;
        const LEFT_VOL  = 0b_0111_0000;
        const VIN_R_EN  = 0b_0000_1000;
        const RIGHT_VOL = 0b_0000_0111;
    }
}

bitflags! {
    // NR51 - Selection of Sound output terminal (R/W)
    struct NR51: u8 {
        const OUT4_L = 0b_1000_0000;
        const OUT3_L = 0b_0100_0000;
        const OUT2_L = 0b_0010_0000;
        const OUT1_L = 0b_0001_0000;
        const OUT4_R = 0b_0000_1000;
        const OUT3_R = 0b_0000_0100;
        const OUT2_R = 0b_0000_0010;
        const OUT1_R = 0b_0000_0001;
    }
}

bitflags! {
    // NR52 - Sound on/off
    struct NR52: u8 {
        const PWR_CTRL = 0b_1000_0000;
        const OUT_4_EN = 0b_0000_1000;
        const OUT_3_EN = 0b_0000_0100;
        const OUT_2_EN = 0b_0000_0010;
        const OUT_1_EN = 0b_0000_0001;
    }
}

/// A sound channel able to produce quadrangular wave patterns
/// with optional sweep and envelope functions.
struct ToneChannel {
    // Channel registers
    nrx0: NRx0,
    nrx1: NRx1,
    nrx2: NRx2,
    nrx3: IoReg<u8>,
    nrx4: NRx4,

    // Internal state and timer counter
    enabled: bool,
    sweep_support: bool,
    timer_counter: u32,

    // Volume control
    volume: i16,
    vol_ctr: u8,
    vol_env_enabled: bool,

    // Channel outpu fed as input to the mixer
    waveform_level: i16,
}

impl ToneChannel {
    /// Creates a tone channel with the initial register state provided.
    fn new(
        nrx0: NRx0,
        nrx1: NRx1,
        nrx2: NRx2,
        nrx3: IoReg<u8>,
        nrx4: NRx4,
        sweep_support: bool,
    ) -> ToneChannel {
        ToneChannel {
            nrx0,
            nrx1,
            nrx2,
            nrx3,
            nrx4,

            enabled: false,
            sweep_support,
            timer_counter: 0,

            volume: 0,
            vol_ctr: 0,
            vol_env_enabled: false,

            waveform_level: 1,
        }
    }

    /// Advances the internal timer state by one M-cycle.
    fn tick(&mut self) {
        let period = (2048 - self.get_frequency()) << 5;

        // The timer generates an output clock every N input clocks,
        // where N is the timer's period.
        if self.timer_counter < 4 {
            self.timer_counter = period - self.timer_counter;
        } else {
            self.timer_counter -= 4;
        }

        // Duty   Waveform    Ratio
        // -------------------------
        // 0      00000001    12.5%
        // 1      10000001    25%
        // 2      10000111    50%
        // 3      01111110    75%
        let threshold = match (self.nrx1 & NRx1::WAVE_DUTY).bits() >> 6 {
            0 => period / 8,
            1 => period / 4,
            2 => period / 2,
            3 => period * 3 / 4,
            _ => unreachable!(),
        };

        self.waveform_level = if self.timer_counter < threshold { 1 } else { 0 };
    }

    /// Advances the volume envelope by 1/64th of a second.
    fn tick_vol_env(&mut self) {
        let period = (self.nrx2 & NRx2::ENV_PERIOD).bits();

        // When the timer generates a clock and the envelope period is not zero,
        // a new volume is calculated by adding or subtracting 1 from the current volume.
        if self.vol_env_enabled && period > 0 {
            self.nrx2 = (self.nrx2 & !NRx2::ENV_PERIOD) | NRx2::from_bits_truncate(period - 1);

            let new_volume = if self.nrx2.contains(NRx2::ENV_DIR) {
                self.volume + 1
            } else {
                self.volume - 1
            };

            // If this new volume within the 0 to 15 range, the volume is updated,
            // otherwise it is left unchanged and no further automatic increments/decrements
            // are made to the volume until the channel is triggered again.
            if new_volume <= 15 {
                self.volume = new_volume;
            } else {
                self.vol_env_enabled = false;
            }
        }
    }

    /// Advances the length counter by 1/256th of a second.
    fn tick_len_ctr(&mut self) {
        let len = (self.nrx1 & NRx1::SOUND_LEN).bits();

        // When clocked while enabled by NRx4 and the counter is not zero, length is decremented
        if self.nrx4.contains(NRx4::LEN_EN) && len != 0 {
            let len = len - 1;

            self.nrx1 = (self.nrx1 & !NRx1::SOUND_LEN) | NRx1::from_bits_truncate(len);

            // If it becomes zero, the channel is disabled
            if len == 0 {
                self.enabled = false;
            }
        }
    }

    /// Returns the channel's current tone frequency.
    fn get_frequency(&self) -> u32 {
        let hi = u32::from((self.nrx4 & NRx4::FREQ_HI).bits());
        let lo = u32::from(self.nrx3.0);
        (hi << 8) | lo
    }

    /// Returns the channel's current volume.
    fn get_volume(&self) -> i16 {
        i16::from(self.enabled) * self.volume
    }

    /// Returns the channel's current output level, ready to be fed to the mixer.
    fn get_channel_out(&self) -> i16 {
        if (self.nrx2 & NRx2::DAC_ON).bits() != 0 {
            (self.waveform_level * 2 * self.get_volume() as i16) - 15
        } else {
            0
        }
    }

    /// Handles a write to the NRx4 register.
    fn write_to_nr4(&mut self, val: u8) {
        self.nrx4 = NRx4::from_bits_truncate(val);

        // When a TRIGGER occurs, a number of things happen
        if self.nrx4.contains(NRx4::TRIGGER) {
            // Channel is enabled
            self.enabled = true;

            // If length counter is zero, it is set to 64 (256 for wave channel)
            if (self.nrx1 & NRx1::SOUND_LEN).bits() == 0 {
                self.nrx1 |= NRx1::SOUND_LEN;
            }

            // Volume envelope timer is reloaded with period and
            // channel volume is reloaded from NRx2.
            self.volume = ((self.nrx2 & NRx2::START_VOL).bits() >> 4) as i16;
            self.vol_ctr = (self.nrx2 & NRx2::ENV_PERIOD).bits();
            self.vol_env_enabled = true;
        }
    }
}

impl MemR for ToneChannel {
    fn read(&self, addr: u16) -> Result<u8, dbg::TraceEvent> {
        Ok(match addr {
            0 => {
                if self.sweep_support {
                    self.nrx0.bits() | 0x80
                } else {
                    0xFF
                }
            }
            1 => self.nrx1.bits() | 0x3F,
            2 => self.nrx2.bits(),
            3 => self.nrx3.0 | 0xFF,
            4 => self.nrx4.bits() | 0xBF,
            _ => unreachable!(),
        })
    }
}

impl MemW for ToneChannel {
    fn write(&mut self, addr: u16, val: u8) -> Result<(), dbg::TraceEvent> {
        match addr {
            0 => self.nrx0 = NRx0::from_bits_truncate(val),
            1 => self.nrx1 = NRx1::from_bits_truncate(val),
            2 => self.nrx2 = NRx2::from_bits_truncate(val),
            3 => self.nrx3.0 = val,
            4 => self.write_to_nr4(val),
            _ => unreachable!(),
        };

        Ok(())
    }
}

struct Mixer {
    // Control registers
    nr50: NR50,
    nr51: NR51,
    nr52: NR52,

    // Audio sample channel
    sample_rate_counter: f32,
    sample_channel: Arc<ArrayQueue<i16>>,
    sample_period: f32,
}

impl Default for Mixer {
    fn default() -> Mixer {
        Mixer {
            nr50: NR50::from_bits_truncate(0x77),
            nr51: NR51::from_bits_truncate(0xF3),
            nr52: NR52::from_bits_truncate(0xF1),

            // Create a sample channel that can hold up to 1024 samples.
            // At 44.1KHz, this is about 23ms worth of audio.
            sample_rate_counter: 0f32,
            sample_channel: Arc::new(ArrayQueue::new(1024)),
            sample_period: std::f32::INFINITY,
        }
    }
}

impl Mixer {
    fn tick(&mut self, ch1: i16, ch2: i16) {
        self.sample_rate_counter += 4.0;

        // Update the audio channel
        if self.sample_rate_counter > self.sample_period {
            self.sample_rate_counter -= self.sample_period;

            let mut so2 = 0;
            let mut so1 = 0;

            // If the peripheral is disabled, no sound is emitted.
            if !self.nr52.contains(NR52::PWR_CTRL) {
                self.sample_channel.push(0).unwrap_or(());
            } else {
                // Update LEFT speaker
                if self.nr51.contains(NR51::OUT1_L) {
                    so2 += ch1;
                }
                if self.nr51.contains(NR51::OUT2_L) {
                    so2 += ch2;
                }

                // Update RIGHT speaker
                if self.nr51.contains(NR51::OUT1_R) {
                    so1 += ch1;
                }
                if self.nr51.contains(NR51::OUT2_R) {
                    so1 += ch2;
                }

                // Adjust master volumes
                so2 *= 1 + ((self.nr50 & NR50::LEFT_VOL).bits() >> 4) as i16;
                so1 *= 1 + (self.nr50 & NR50::RIGHT_VOL).bits() as i16;

                // Produce a sample which is an average of the two channels.
                // TODO implement true stero sound.
                self.sample_channel.push((so1 + so2) / 2).unwrap_or(());
            }
        }
    }
}

pub struct APU {
    // Channels 1/2
    ch1: ToneChannel,
    ch2: ToneChannel,

    // Channel 3 registers
    ch3_snd_reg: IoReg<u8>,
    ch3_len_reg: IoReg<u8>,
    ch3_vol_reg: IoReg<u8>,
    ch3_flo_reg: IoReg<u8>,
    ch3_fhi_reg: IoReg<u8>,

    // Channel 4 registers
    ch4_len_reg: IoReg<u8>,
    ch4_vol_reg: IoReg<u8>,
    ch4_cnt_reg: IoReg<u8>,
    ch4_ini_reg: IoReg<u8>,

    // Mixer
    mixer: Mixer,

    wave_ram: [u8; 16],

    // Frame sequencer clocks
    clk_64: u32,
    clk_128: u32,
    clk_256: u32,
}

impl Default for APU {
    fn default() -> APU {
        APU {
            ch1: ToneChannel::new(
                NRx0::from_bits_truncate(0x80),
                NRx1::from_bits_truncate(0x8F),
                NRx2::from_bits_truncate(0xF3),
                IoReg(0x00),
                NRx4::from_bits_truncate(0xBF),
                true,
            ),

            ch2: ToneChannel::new(
                NRx0::from_bits_truncate(0xFF),
                NRx1::from_bits_truncate(0x3F),
                NRx2::from_bits_truncate(0x00),
                IoReg(0x00),
                NRx4::from_bits_truncate(0xBF),
                false,
            ),

            ch3_snd_reg: IoReg(0x7F),
            ch3_len_reg: IoReg(0xFF),
            ch3_vol_reg: IoReg(0x9F),
            ch3_flo_reg: IoReg(0x00),
            ch3_fhi_reg: IoReg(0xBF),

            ch4_len_reg: IoReg(0xFF),
            ch4_vol_reg: IoReg(0x00),
            ch4_cnt_reg: IoReg(0x00),
            ch4_ini_reg: IoReg(0xBF),

            mixer: Mixer::default(),

            wave_ram: [0; 16],

            // TODO according to [1] these clocks are slightly out of phase,
            // initialization and ticking should be fixed accordingly.
            // [1] http://gbdev.gg8.se/wiki/articles/Gameboy_sound_hardware#Frame_Sequencer
            clk_64: CLK_64_RELOAD,
            clk_128: CLK_128_RELOAD,
            clk_256: CLK_256_RELOAD,
        }
    }
}

impl APU {
    /// Instantiates a new APU producing samples at a frequency of `sample_rate`.
    pub fn new(sample_rate: f32) -> APU {
        let mut apu = APU::default();
        apu.set_sample_rate(sample_rate);
        apu
    }

    /// Advances the sound controller state machine by a single M-cycle.
    pub fn tick(&mut self) {
        self.clk_64 -= 4;
        self.clk_128 -= 4;
        self.clk_256 -= 4;

        // Internal timer clock tick
        self.ch1.tick();
        self.ch2.tick();

        // Volume envelope clock tick
        if self.clk_64 == 0 {
            self.clk_64 = CLK_64_RELOAD;

            self.ch1.tick_vol_env();
            self.ch2.tick_vol_env();
        }

        // Sweep clock tick
        if self.clk_128 == 0 {
            self.clk_128 = CLK_128_RELOAD;
        }

        // Lenght counter clock tick
        if self.clk_256 == 0 {
            self.clk_256 = CLK_256_RELOAD;

            self.ch1.tick_len_ctr();
            self.ch2.tick_len_ctr();
        }

        self.mixer
            .tick(self.ch1.get_channel_out(), self.ch2.get_channel_out());
    }

    /// Changes the current sample rate.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.mixer.sample_period = (crate::CPU_CLOCK as f32) / sample_rate;
        self.mixer.sample_rate_counter = 0f32;
    }

    /// Returns a copy of the audio sample channel.
    pub fn get_sample_channel(&self) -> Arc<ArrayQueue<i16>> {
        self.mixer.sample_channel.clone()
    }
}

impl InterruptSource for APU {
    fn get_and_clear_irq(&mut self) -> Option<IrqSource> {
        None
    }
}

impl MemR for APU {
    fn read(&self, addr: u16) -> Result<u8, dbg::TraceEvent> {
        Ok(match addr {
            0xFF10..=0xFF14 => self.ch1.read(addr - 0xFF10)?,
            0xFF15..=0xFF19 => self.ch2.read(addr - 0xFF15)?,

            0xFF1A => self.ch3_snd_reg.0 | 0x7F,
            0xFF1B => self.ch3_len_reg.0,
            0xFF1C => self.ch3_vol_reg.0 | 0x9F,
            0xFF1D => self.ch3_flo_reg.0 | 0xFF,
            0xFF1E => self.ch3_fhi_reg.0 | 0xBF,

            0xFF20 => self.ch4_len_reg.0 | 0xC0,
            0xFF21 => self.ch4_vol_reg.0,
            0xFF22 => self.ch4_cnt_reg.0,
            0xFF23 => self.ch4_ini_reg.0 | 0xBF,

            0xFF24 => self.mixer.nr50.bits(),
            0xFF25 => self.mixer.nr51.bits(),
            0xFF26 => self.mixer.nr52.bits() | 0x70,

            0xFF30..=0xFF3F => self.wave_ram[usize::from(addr) - 0xFF30],

            // Unused regs in this range: 0xFF15, 0xFF1F, 0xFF27..=0xFF2F
            _ => 0xFF,
        })
    }
}

impl MemW for APU {
    fn write(&mut self, addr: u16, val: u8) -> Result<(), dbg::TraceEvent> {
        match addr {
            0xFF10..=0xFF14 => self.ch1.write(addr - 0xFF10, val)?,
            0xFF15..=0xFF19 => self.ch2.write(addr - 0xFF15, val)?,

            0xFF1A => self.ch3_snd_reg.0 = val,
            0xFF1B => self.ch3_len_reg.0 = val,
            0xFF1C => self.ch3_vol_reg.0 = val,
            0xFF1D => self.ch3_flo_reg.0 = val,
            0xFF1E => self.ch3_fhi_reg.0 = val,

            0xFF20 => self.ch4_len_reg.0 = val,
            0xFF21 => self.ch4_vol_reg.0 = val,
            0xFF22 => self.ch4_cnt_reg.0 = val,
            0xFF23 => self.ch4_ini_reg.0 = val,

            0xFF24 => self.mixer.nr50 = NR50::from_bits_truncate(val),
            0xFF25 => self.mixer.nr51 = NR51::from_bits_truncate(val),
            0xFF26 => self.mixer.nr52 = NR52::from_bits_truncate(val),

            0xFF30..=0xFF3F => self.wave_ram[usize::from(addr) - 0xFF30] = val,

            // Unused regs in this range: 0xFF15, 0xFF1F, 0xFF27..=0xFF2F
            _ => (),
        };

        Ok(())
    }
}
