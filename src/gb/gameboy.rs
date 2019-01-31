use super::bus::Bus;
use super::cpu::CPU;

const CPU_CLOCK: u64 = 4_194_304; // Hz
const HSYNC_CLOCK: u64 = 9_198; // Hz

const CYCLES_PER_HSYNC: u64 = CPU_CLOCK / HSYNC_CLOCK;

pub struct GameBoy {
    cpu: CPU,
    bus: Bus,
}

impl GameBoy {
    pub fn with_cartridge(rom: &[u8]) -> GameBoy {
        GameBoy {
            cpu: CPU::new(),
            bus: Bus::new(rom),
        }
    }

    pub fn step(&mut self) {
        let elapsed = {
            let clk = self.cpu.clk;

            if !self.cpu.halted {
                self.cpu.exec(&mut self.bus);
            } else {
                self.cpu.clk += 4;
            }
            self.cpu.clk - clk
        };

        self.bus.ppu.tick(elapsed);
    }

    pub fn run_for_vblank(&mut self) {
        let until_clk = self.cpu.clk + CYCLES_PER_HSYNC * 154;

        while self.cpu.clk < until_clk {
            self.step();
        }
    }

    pub fn rasterize(&self, vbuf: &mut [u8]) {
        self.bus.ppu.rasterize(vbuf);
    }

    pub fn cpu(&self) -> &CPU {
        &self.cpu
    }

    pub fn bus(&self) -> &Bus {
        &self.bus
    }
}
