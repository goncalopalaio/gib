use super::dbg;
use super::io::{IrqController, Joypad, Serial, Timer, APU, PPU};
use super::mem::{MemR, MemRW, MemSize, MemW, Memory};

use std::convert::TryFrom;

const BOOT_ROM: [u8; 256] = [
    0x31, 0xfe, 0xff, 0xaf, 0x21, 0xff, 0x9f, 0x32, 0xcb, 0x7c, 0x20, 0xfb, 0x21, 0x26, 0xff, 0x0e,
    0x11, 0x3e, 0x80, 0x32, 0xe2, 0x0c, 0x3e, 0xf3, 0xe2, 0x32, 0x3e, 0x77, 0x77, 0x3e, 0xfc, 0xe0,
    0x47, 0x11, 0x04, 0x01, 0x21, 0x10, 0x80, 0x1a, 0xcd, 0x95, 0x00, 0xcd, 0x96, 0x00, 0x13, 0x7b,
    0xfe, 0x34, 0x20, 0xf3, 0x11, 0xd8, 0x00, 0x06, 0x08, 0x1a, 0x13, 0x22, 0x23, 0x05, 0x20, 0xf9,
    0x3e, 0x19, 0xea, 0x10, 0x99, 0x21, 0x2f, 0x99, 0x0e, 0x0c, 0x3d, 0x28, 0x08, 0x32, 0x0d, 0x20,
    0xf9, 0x2e, 0x0f, 0x18, 0xf3, 0x67, 0x3e, 0x64, 0x57, 0xe0, 0x42, 0x3e, 0x91, 0xe0, 0x40, 0x04,
    0x1e, 0x02, 0x0e, 0x0c, 0xf0, 0x44, 0xfe, 0x90, 0x20, 0xfa, 0x0d, 0x20, 0xf7, 0x1d, 0x20, 0xf2,
    0x0e, 0x13, 0x24, 0x7c, 0x1e, 0x83, 0xfe, 0x62, 0x28, 0x06, 0x1e, 0xc1, 0xfe, 0x64, 0x20, 0x06,
    0x7b, 0xe2, 0x0c, 0x3e, 0x87, 0xe2, 0xf0, 0x42, 0x90, 0xe0, 0x42, 0x15, 0x20, 0xd2, 0x05, 0x20,
    0x4f, 0x16, 0x20, 0x18, 0xcb, 0x4f, 0x06, 0x04, 0xc5, 0xcb, 0x11, 0x17, 0xc1, 0xcb, 0x11, 0x17,
    0x05, 0x20, 0xf5, 0x22, 0x23, 0x22, 0x23, 0xc9, 0xce, 0xed, 0x66, 0x66, 0xcc, 0x0d, 0x00, 0x0b,
    0x03, 0x73, 0x00, 0x83, 0x00, 0x0c, 0x00, 0x0d, 0x00, 0x08, 0x11, 0x1f, 0x88, 0x89, 0x00, 0x0e,
    0xdc, 0xcc, 0x6e, 0xe6, 0xdd, 0xdd, 0xd9, 0x99, 0xbb, 0xbb, 0x67, 0x63, 0x6e, 0x0e, 0xec, 0xcc,
    0xdd, 0xdc, 0x99, 0x9f, 0xbb, 0xb9, 0x33, 0x3e, 0x3c, 0x42, 0xb9, 0xa5, 0xb9, 0xa5, 0x42, 0x3c,
    0x21, 0x04, 0x01, 0x11, 0xa8, 0x00, 0x1a, 0x13, 0xbe, 0x20, 0xfe, 0x23, 0x7d, 0xfe, 0x34, 0x20,
    0xf5, 0x06, 0x19, 0x78, 0x86, 0x23, 0x05, 0x20, 0xfb, 0x86, 0x20, 0xfe, 0x3e, 0x01, 0xe0, 0x50,
];

pub enum MbcType {
    None,
    MBC1,
}

pub struct McbTypeError(u8);

impl TryFrom<u8> for MbcType {
    type Error = McbTypeError;

    fn try_from(n: u8) -> Result<Self, Self::Error> {
        match n {
            0x00 => Ok(MbcType::None),
            0x01..=0x03 => Ok(MbcType::MBC1),
            _ => Err(McbTypeError(n)),
        }
    }
}

pub struct Bus {
    rom_banks: Vec<Memory>,

    pub rom_nn: usize,
    rom_backup: [u8; 256],

    pub eram: Memory,
    pub hram: Memory,
    pub wram_00: Memory,
    pub wram_nn: Memory,

    pub apu: APU,
    pub ppu: PPU,
    pub tim: Timer,
    pub sdt: Serial,
    pub pad: Joypad,
    pub itr: IrqController,

    mbc: MbcType,
}

impl Default for Bus {
    fn default() -> Bus {
        Bus {
            rom_banks: vec![],

            rom_nn: 1,
            rom_backup: [0; 256],

            eram: Memory::new(0x2000),
            hram: Memory::new(127),
            wram_00: Memory::new(0x1000),
            wram_nn: Memory::new(0x1000),

            apu: APU::new(),
            ppu: PPU::new(),
            tim: Timer::new(),
            sdt: Serial::new(),
            pad: Joypad::new(),
            itr: IrqController::new(),

            mbc: MbcType::None,
        }
    }
}

impl Bus {
    pub fn new() -> Bus {
        Bus::default()
    }

    pub fn load_rom(&mut self, rom: &[u8]) -> Result<(), dbg::TraceEvent> {
        for chunk in rom.chunks(0x4000) {
            let mut mem = Memory::new(0x4000);

            for (i, b) in chunk.iter().enumerate() {
                mem.write(i as u16, *b)?;
            }
            self.rom_banks.push(mem);
        }

        // Check MBC type in the ROM header
        self.mbc = MbcType::try_from(rom[0x147])
            .map_err(|McbTypeError(n)| dbg::TraceEvent::UnsupportedMbcType(n))?;

        self.enable_bootrom()
    }

    fn enable_bootrom(&mut self) -> Result<(), dbg::TraceEvent> {
        for i in 0u16..256 {
            self.rom_backup[usize::from(i)] = self.rom_banks[0].read(i)?;
            self.rom_banks[0].write(i, BOOT_ROM[usize::from(i)])?;
        }
        Ok(())
    }

    fn disable_bootrom(&mut self) -> Result<(), dbg::TraceEvent> {
        for i in 0u16..256 {
            self.rom_banks[0].write(i, self.rom_backup[usize::from(i)])?;
        }
        Ok(())
    }

    fn ram_enable<T: MemSize>(&mut self, _val: T) -> Result<(), dbg::TraceEvent> {
        // TODO handle this just in case some ROMs rely on uncorrect behavior
        Ok(())
    }

    fn rom_select<T: MemSize>(&mut self, val: T) -> Result<(), dbg::TraceEvent> {
        self.rom_nn = match val.low() {
            0x00 => 0x01,
            v @ 0x01..=0x1F => usize::from(v),
            v => return Err(dbg::TraceEvent::InvalidMbcOp(dbg::McbOp::RomBank, v)),
        };
        Ok(())
    }

    fn ram_rom_select<T: MemSize>(&mut self, val: T) -> Result<(), dbg::TraceEvent> {
        Err(dbg::TraceEvent::InvalidMbcOp(
            dbg::McbOp::RamBank,
            val.low(),
        ))
    }

    fn mode_select<T: MemSize>(&mut self, val: T) -> Result<(), dbg::TraceEvent> {
        Err(dbg::TraceEvent::InvalidMbcOp(
            dbg::McbOp::RamBank,
            val.low(),
        ))
    }

    fn write_to_cgb_functions<T: MemSize>(
        &mut self,
        addr: u16,
        _val: T,
    ) -> Result<(), dbg::TraceEvent> {
        match addr {
            0xFF4D => Err(dbg::TraceEvent::CgbSpeedSwitchReq),
            _ => Err(dbg::TraceEvent::UnsupportedCgbOp(addr)),
        }
    }
}

impl MemR for Bus {
    fn read<T: MemSize>(&self, addr: u16) -> Result<T, dbg::TraceEvent> {
        match addr {
            0x0000..=0x3FFF => self.rom_banks[0].read(addr),
            0x4000..=0x7FFF => self.rom_banks[self.rom_nn].read(addr - 0x4000),
            0x8000..=0x9FFF => self.ppu.read(addr),
            0xA000..=0xBFFF => self.eram.read(addr - 0xA000),
            0xC000..=0xCFFF => self.wram_00.read(addr - 0xC000),
            0xD000..=0xDFFF => self.wram_nn.read(addr - 0xD000),
            0xE000..=0xEFFF => self.wram_00.read(addr - 0xE000),
            0xF000..=0xFDFF => self.wram_nn.read(addr - 0xF000),
            0xFE00..=0xFE9F => self.ppu.read(addr),
            0xFF00..=0xFF00 => self.pad.read(addr),
            0xFF01..=0xFF02 => self.sdt.read(addr),
            0xFF04..=0xFF07 => self.tim.read(addr),
            0xFF10..=0xFF3F => self.apu.read(addr),
            0xFF40..=0xFF4F => self.ppu.read(addr),
            0xFF51..=0xFF6F => self.ppu.read(addr),
            0xFF80..=0xFFFE => self.hram.read(addr - 0xFF80),
            0xFF0F | 0xFFFF => self.itr.read(addr),
            _ => Err(dbg::TraceEvent::BusFault(addr)),
        }
    }
}

impl MemW for Bus {
    fn write<T: MemSize>(&mut self, addr: u16, val: T) -> Result<(), dbg::TraceEvent> {
        match addr {
            0x0000..=0x1FFF => self.ram_enable(val),
            0x2000..=0x3FFF => self.rom_select(val),
            0x4000..=0x5FFF => self.ram_rom_select(val),
            0x6000..=0x7FFF => self.mode_select(val),
            0x8000..=0x9FFF => self.ppu.write(addr, val),
            0xA000..=0xBFFF => self.eram.write(addr - 0xA000, val),
            0xC000..=0xCFFF => self.wram_00.write(addr - 0xC000, val),
            0xD000..=0xDFFF => self.wram_nn.write(addr - 0xD000, val),
            0xE000..=0xEFFF => self.wram_00.write(addr - 0xE000, val),
            0xF000..=0xFDFF => self.wram_nn.write(addr - 0xF000, val),
            0xFE00..=0xFE9F => self.ppu.write(addr, val),
            0xFEA0..=0xFEFF => Ok(()), /* Writing to unused memory is a no-op */
            0xFF00..=0xFF00 => self.pad.write(addr, val),
            0xFF01..=0xFF02 => self.sdt.write(addr, val),
            0xFF04..=0xFF07 => self.tim.write(addr, val),
            0xFF10..=0xFF3F => self.apu.write(addr, val),
            0xFF40..=0xFF4B => self.ppu.write(addr, val),
            0xFF4D..=0xFF4F => self.write_to_cgb_functions(addr, val),
            0xFF51..=0xFF6F => self.ppu.write(addr, val),
            0xFF70..=0xFF7F => self.write_to_cgb_functions(addr, val),
            0xFF80..=0xFFFE => self.hram.write(addr - 0xFF80, val),
            0xFF0F | 0xFFFF => self.itr.write(addr, val),
            0xFF50 => self.disable_bootrom(),
            _ => Err(dbg::TraceEvent::BusFault(addr)),
        }
    }
}

impl MemRW for Bus {}
