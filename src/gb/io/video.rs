use super::dbg;
use super::{InterruptSource, IrqSource};
use super::{IoReg, MemR, MemRW, MemSize, MemW};

#[derive(Default, Copy, Clone)]
struct Tile([u8; 16]);

impl Tile {
    fn data(&self) -> &[u8] {
        &self.0[..]
    }

    fn data_mut(&mut self) -> &mut [u8] {
        &mut self.0[..]
    }

    pub fn pixel(&self, x: u8, y: u8) -> u8 {
        let bl = self.0[usize::from(y) * 2];
        let bh = self.0[usize::from(y) * 2 + 1];
        (((bh >> (7 - x)) & 0x1) << 1) | ((bl >> (7 - x)) & 0x1)
    }
}

#[derive(Default, Copy, Clone)]
struct Sprite([u8; 4]);

impl Sprite {
    fn data(&self) -> &[u8] {
        &self.0[..]
    }

    fn data_mut(&mut self) -> &mut [u8] {
        &mut self.0[..]
    }
}

impl<'a> MemR for &'a [Sprite] {
    fn read<T: MemSize>(&self, addr: u16) -> Result<T, dbg::TraceEvent> {
        let s = &self[usize::from(addr >> 2)];
        T::read_le(&s.data()[usize::from(addr % 2)..])
    }
}

impl<'a> MemR for &'a mut [Sprite] {
    fn read<T: MemSize>(&self, addr: u16) -> Result<T, dbg::TraceEvent> {
        (&*self as &[Sprite]).read(addr)
    }
}

impl<'a> MemW for &'a mut [Sprite] {
    fn write<T: MemSize>(&mut self, addr: u16, val: T) -> Result<(), dbg::TraceEvent> {
        let s = &mut self[usize::from(addr >> 2)];
        T::write_le(&mut s.data_mut()[usize::from(addr % 2)..], val)
    }
}

impl<'a> MemRW for &'a mut [Sprite] {}

pub struct PPU {
    tdt: [Tile; 384],  // Tile Data Table
    oam: [Sprite; 40], // Object Attribute Memory
    bgtm0: [u8; 1024], // Background Tile Map #0
    bgtm1: [u8; 1024], // Background Tile Map #1

    // Ctrl/status IO registes
    lcdc_reg: IoReg<u8>,
    stat_reg: IoReg<u8>,

    // Position/scrolling registers
    scx_reg: IoReg<u8>,
    scy_reg: IoReg<u8>,
    lyc_reg: IoReg<u8>,
    ly_reg: IoReg<u8>,
    wy_reg: IoReg<u8>,
    wx_reg: IoReg<u8>,

    // Monochorome palette registers
    obp0_reg: IoReg<u8>,
    obp1_reg: IoReg<u8>,
    bgp_reg: IoReg<u8>,

    // DMA register
    dma_reg: IoReg<u8>,

    // Timings
    tstate: u64,

    // IRQ handling
    vblank_irq_pending: bool,
    stat_irq_pending: bool,
}

impl Default for PPU {
    fn default() -> PPU {
        PPU {
            tdt: [Tile::default(); 384],
            oam: [Sprite::default(); 40],
            bgtm0: [0; 1024],
            bgtm1: [0; 1024],

            lcdc_reg: IoReg(0x00),
            stat_reg: IoReg(0x00),

            scx_reg: IoReg(0x00),
            scy_reg: IoReg(0x00),
            lyc_reg: IoReg(0x00),
            ly_reg: IoReg(0x00),
            wy_reg: IoReg(0x00),
            wx_reg: IoReg(0x00),

            obp0_reg: IoReg(0x00),
            obp1_reg: IoReg(0x00),
            bgp_reg: IoReg(0x00),

            dma_reg: IoReg(0x00),

            tstate: 0,

            vblank_irq_pending: false,
            stat_irq_pending: false,
        }
    }
}

impl PPU {
    pub fn new() -> PPU {
        PPU::default()
    }

    pub fn rasterize(&self, vbuf: &mut [u8]) {
        if !self.lcdc_reg.bit(7) {
            for b in vbuf.iter_mut() {
                *b = 0xFF;
            }
            return;
        }

        self.rasterize_bg(vbuf);
    }

    fn rasterize_bg(&self, vbuf: &mut [u8]) {
        if !self.lcdc_reg.bit(0) {
            for b in vbuf.iter_mut() {
                *b = 0xFF;
            }
            return;
        }

        for py in 0usize..144 {
            for px in 0usize..160 {
                let y = (py + usize::from(self.scy_reg.0)) % 256;
                let x = (px + usize::from(self.scx_reg.0)) % 256;

                let pid = (py * (160 * 4)) + (px * 4);

                let t = self.bg_tile(((y >> 3) << 5) + (x >> 3));
                let px = t.pixel((x & 0x07) as u8, (y & 0x7) as u8);
                let shade = self.shade(px);

                vbuf[pid] = shade;
                vbuf[pid + 1] = shade;
                vbuf[pid + 2] = shade;
            }
        }
    }

    pub fn tick(&mut self) {
        if !self.lcdc_reg.bit(7) {
            return;
        }

        self.tstate = (self.tstate + 4) % 70224;

        let tstate = self.tstate % 456;
        let v_line = self.tstate / 456;

        let mode = if v_line < 144 {
            match tstate {
                0..=79 => 2,   // Mode 2
                80..=253 => 3, // Mode 3
                _ => 0,        // Mode 0
            }
        } else {
            1
        };

        self.stat_reg.0 = (self.stat_reg.0 & (!0x3)) | mode;
        self.ly_reg.0 = v_line as u8;

        // V-Blank IRQ happens at the beginning of the 144th line
        if v_line == 144 && tstate == 0 {
            self.vblank_irq_pending = true;
        }
    }

    fn shade(&self, color: u8) -> u8 {
        match (self.bgp_reg.0 >> (color * 2)) & 0x3 {
            0b00 => 0xFF,
            0b01 => 0xAA,
            0b10 => 0x55,
            0b11 => 0x00,
            _ => unreachable!(),
        }
    }

    fn bg_tile(&self, id: usize) -> &Tile {
        let tile_id = if self.lcdc_reg.bit(3) {
            self.bgtm1[id]
        } else {
            self.bgtm0[id]
        };

        if self.lcdc_reg.bit(4) {
            &self.tdt[usize::from(tile_id)]
        } else {
            &self.tdt[(128 + i32::from(tile_id as i8)) as usize]
        }
    }
}

impl InterruptSource for PPU {
    fn get_and_clear_irq(&mut self) -> Option<IrqSource> {
        if self.vblank_irq_pending {
            self.vblank_irq_pending = false;
            Some(IrqSource::VBlank)
        } else if self.stat_irq_pending {
            self.stat_irq_pending = false;
            Some(IrqSource::LcdStat)
        } else {
            None
        }
    }
}

impl MemR for PPU {
    fn read<T: MemSize>(&self, addr: u16) -> Result<T, dbg::TraceEvent> {
        match addr {
            0x8000..=0x97FF => {
                let addr = addr - 0x8000;
                let tid = usize::from(addr >> 4);
                let bid = usize::from(addr & 0xF);
                T::read_le(&self.tdt[tid].data()[bid..])
            }
            0x9800..=0x9BFF => T::read_le(&self.bgtm0[usize::from(addr - 0x9800)..]),
            0x9C00..=0x9FFF => T::read_le(&self.bgtm1[usize::from(addr - 0x9C00)..]),

            0xFE00..=0xFE9F => (&self.oam[..]).read(addr - 0xFE00),

            0xFF40 => T::read_le(&[self.lcdc_reg.0]),
            0xFF41 => T::read_le(&[self.stat_reg.0 | 0x80]),
            0xFF42 => T::read_le(&[self.scy_reg.0]),
            0xFF43 => T::read_le(&[self.scx_reg.0]),
            0xFF44 => T::read_le(&[self.ly_reg.0]),
            0xFF45 => T::read_le(&[self.lyc_reg.0]),
            0xFF46 => T::read_le(&[0xFF]),
            0xFF47 => T::read_le(&[self.bgp_reg.0]),
            0xFF48 => T::read_le(&[self.obp0_reg.0]),
            0xFF49 => T::read_le(&[self.obp1_reg.0]),
            0xFF4A => T::read_le(&[self.wy_reg.0]),
            0xFF4B => T::read_le(&[self.wx_reg.0]),

            _ => unreachable!(),
        }
    }
}

impl MemW for PPU {
    fn write<T: MemSize>(&mut self, addr: u16, val: T) -> Result<(), dbg::TraceEvent> {
        match addr {
            0x8000..=0x97FF => {
                let addr = addr - 0x8000;
                let tid = usize::from(addr >> 4);
                let bid = usize::from(addr & 0xF);
                T::write_le(&mut self.tdt[tid].data_mut()[bid..], val)
            }
            0x9800..=0x9BFF => T::write_le(&mut self.bgtm0[usize::from(addr - 0x9800)..], val),
            0x9C00..=0x9FFF => T::write_le(&mut self.bgtm1[usize::from(addr - 0x9C00)..], val),

            0xFE00..=0xFE9F => (&mut self.oam[..]).write(addr - 0xFE00, val),

            0xFF40 => T::write_mut_le(&mut [&mut self.lcdc_reg.0], val),
            0xFF41 => T::write_mut_le(&mut [&mut self.stat_reg.0], val),
            0xFF42 => T::write_mut_le(&mut [&mut self.scy_reg.0], val),
            0xFF43 => T::write_mut_le(&mut [&mut self.scx_reg.0], val),
            0xFF44 => Ok(()),
            0xFF45 => T::write_mut_le(&mut [&mut self.lyc_reg.0], val),
            0xFF46 => T::write_mut_le(&mut [&mut self.dma_reg.0], val),
            0xFF47 => T::write_mut_le(&mut [&mut self.bgp_reg.0], val),
            0xFF48 => T::write_mut_le(&mut [&mut self.obp0_reg.0], val),
            0xFF49 => T::write_mut_le(&mut [&mut self.obp1_reg.0], val),
            0xFF4A => T::write_mut_le(&mut [&mut self.wy_reg.0], val),
            0xFF4B => T::write_mut_le(&mut [&mut self.wx_reg.0], val),

            _ => unreachable!(),
        }
    }
}
