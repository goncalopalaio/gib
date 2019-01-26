use super::bus::{Bus, MemR, MemW};
use super::CPU;

macro_rules! pop {
    ($cpu:ident, $bus:ident, $cond:expr, $reg:ident) => {{
        if $cond {
            $cpu.$reg = $bus.read($cpu.sp);
            $cpu.sp += 2;
            $cpu.clk += 8;
        }
    }};
}

macro_rules! push {
    ($cpu:ident, $bus:ident, $reg:ident) => {{
        $cpu.sp -= 2;
        $cpu.clk += 12;
        $bus.write($cpu.sp, $cpu.$reg);
    }};
}

macro_rules! jp {
    ($cpu:ident, $cond:expr, $abs:expr) => {{
        if $cond {
            $cpu.pc = $abs;
            $cpu.clk += 4;
        }
    }};
}

macro_rules! jr {
    ($cpu:ident, $cond:expr, $offset:expr) => {
        jp!(
            $cpu,
            $cond,
            (i32::from($cpu.pc) + i32::from($offset)) as u16
        );
    };
}

macro_rules! call {
    ($cpu:ident, $bus:ident, $cond:expr, $to:expr) => {{
        if $cond {
            push!($cpu, $bus, pc);
            $cpu.pc = $to;
        }
    }};
}

macro_rules! logical {
    ($cpu:ident, $op:tt, $rhs:expr, $sf:expr, $hc: expr, $cy:expr) => {{
        $cpu.set_a($cpu.a() $op $rhs);

        $cpu.set_zf($cpu.a() == 0);
        $cpu.set_sf($sf != 0);
        $cpu.set_hc($hc != 0);
        $cpu.set_cy($cy != 0);
    }};
}

macro_rules! and { ($cpu:ident, $rhs:expr) => { logical!($cpu, &, $rhs, 0, 1, 0); }; }
macro_rules! xor { ($cpu:ident, $rhs:expr) => { logical!($cpu, ^, $rhs, 0, 0, 0); }; }
macro_rules! or  { ($cpu:ident, $rhs:expr) => { logical!($cpu, |, $rhs, 0, 0, 0); }; }

macro_rules! inc {
    ($cpu:ident, $v:expr) => {{
        $cpu.set_zf(($v + 1) == 0);
        $cpu.set_sf(false);
        $cpu.set_hc(($v & 0xF) == 0xF);
        $v + 1
    }};
}

macro_rules! dec {
    ($cpu:ident, $v:expr) => {{
        $cpu.set_zf(($v - 1) == 0);
        $cpu.set_sf(true);
        $cpu.set_hc($v.trailing_zeros() >= 4);
        $v - 1
    }};
}

macro_rules! add {
    ($cpu:ident, $v:expr) => {{
        let old = $cpu.a();
        $cpu.set_a(old + $v);

        $cpu.set_zf($cpu.a() == 0);
        $cpu.set_sf(false);
        $cpu.set_hc((old & 0xF) + ($v & 0xF) >= 0x10);
        $cpu.set_cy($cpu.a() < old);
    }};
}

macro_rules! sub {
    ($cpu:ident, $v:expr) => {{
        let old = $cpu.a();
        $cpu.set_a(old - $v);

        cmp!($cpu, old, $v);
    }};
}

macro_rules! add16 {
    ($cpu:ident, $dst: expr, $v:expr) => {{
        let old = $dst;
        $dst += $v;

        $cpu.set_sf(false);
        $cpu.set_hc((old & 0xF) + ($v & 0xF) >= 0x10);
        $cpu.set_cy($dst < old);
    }};
}

macro_rules! cmp {
    ($cpu:ident, $a:expr, $b:expr) => {{
        $cpu.set_zf($a == $b);
        $cpu.set_sf(true);
        $cpu.set_hc(($b & 0xF) > ($a & 0xF));
        $cpu.set_cy($b > $a);
        $cpu.clk += 4;
    }};
}

macro_rules! rl {
    ($cpu:ident, $cy:expr, $v:expr) => {{
        let cy = $v >> 7;
        let res = ($v << 1) | if $cy { cy } else { u8::from($cpu.cy()) };

        $cpu.set_zf(res == 0);
        $cpu.set_sf(false);
        $cpu.set_hc(false);
        $cpu.set_cy(cy != 0);
        res
    }};
}

macro_rules! rr {
    ($cpu:ident, $cy:expr, $v:expr) => {{
        let cy = $v & 0x1;
        let res = ($v >> 1) | (if $cy { cy } else { u8::from($cpu.cy()) } << 7);

        $cpu.set_zf(res == 0);
        $cpu.set_sf(false);
        $cpu.set_hc(false);
        $cpu.set_cy(cy != 0);
        res
    }};
}

macro_rules! bit {
    ($cpu:ident, $n:expr, $v:expr) => {{
        $cpu.set_zf(($v & (1 << $n)) == 0);
        $cpu.set_sf(false);
        $cpu.set_hc(true);
    }};
}

macro_rules! res {
    ($n:expr, $v:expr) => {
        $v & (!(1 << $n))
    };
}

macro_rules! set {
    ($n:expr, $v:expr) => {
        $v | (1 << $n)
    };
}

impl CPU {
    #[rustfmt::skip]
    #[allow(clippy::cyclomatic_complexity)]
    pub fn op(&mut self, bus: &mut Bus, opcode: u8) {
        match opcode {
            /*
             * Misc/control instructions
             */
            0x00 => (),

            0x10 | 0x76 => self.halted = true,
            0xF3 => self.intr_enabled = false,
            0xFB => self.intr_enabled = true,

            0xCB => {
                let cb = self.fetch::<u8>(bus);
                self.op_cb(bus, cb);
            }

            /*
             * Jump/calls
             */
            0x20 => { let off = self.fetch::<i8>(bus); jr!(self, !self.zf(), off); }
            0x30 => { let off = self.fetch::<i8>(bus); jr!(self, !self.cy(), off); }
            0x28 => { let off = self.fetch::<i8>(bus); jr!(self, self.zf(),  off); }
            0x38 => { let off = self.fetch::<i8>(bus); jr!(self, self.cy(),  off); }
            0x18 => { let off = self.fetch::<i8>(bus); jr!(self, true,       off); }

            0xC2 => { let abs = self.fetch::<u16>(bus); jp!(self, !self.zf(), abs); }
            0xD2 => { let abs = self.fetch::<u16>(bus); jp!(self, !self.cy(), abs); }
            0xCA => { let abs = self.fetch::<u16>(bus); jp!(self, self.zf(),  abs); }
            0xDA => { let abs = self.fetch::<u16>(bus); jp!(self, self.cy(),  abs); }
            0xC3 => { let abs = self.fetch::<u16>(bus); jp!(self, true,       abs); }

            0xE9 => jp!(self, true, self.hl),

            0xC4 => { let abs = self.fetch::<u16>(bus); call!(self, bus, !self.zf(), abs); }
            0xD4 => { let abs = self.fetch::<u16>(bus); call!(self, bus, !self.cy(), abs); }
            0xCC => { let abs = self.fetch::<u16>(bus); call!(self, bus, self.zf(),  abs); }
            0xDC => { let abs = self.fetch::<u16>(bus); call!(self, bus, self.cy(),  abs); }
            0xCD => { let abs = self.fetch::<u16>(bus); call!(self, bus, true,       abs); }

            0xC0 => pop!(self, bus, !self.zf(), pc),
            0xD0 => pop!(self, bus, !self.cy(), pc),
            0xC8 => pop!(self, bus, self.zf(),  pc),
            0xD8 => pop!(self, bus, self.cy(),  pc),
            0xC9 => pop!(self, bus, true,       pc),

            0xD9 => { pop!(self, bus, true, pc); self.intr_enabled = true; }

            0xC7 => call!(self, bus, true, 0x00),
            0xCF => call!(self, bus, true, 0x08),
            0xD7 => call!(self, bus, true, 0x10),
            0xDF => call!(self, bus, true, 0x18),
            0xE7 => call!(self, bus, true, 0x20),
            0xEF => call!(self, bus, true, 0x28),
            0xF7 => call!(self, bus, true, 0x30),
            0xFF => call!(self, bus, true, 0x38),

            /*
             * 8bit load/store/move instructions
             */
            0x02 => bus.write(self.bc, self.a()),
            0x12 => bus.write(self.de, self.a()),

            0x22 => { bus.write(self.hl, self.a()); self.hl += 1; }
            0x32 => { bus.write(self.hl, self.a()); self.hl -= 1; }

            0x0A => self.set_a(bus.read(self.bc)),
            0x1A => self.set_a(bus.read(self.de)),

            0x2A => { self.set_a(bus.read(self.hl)); self.hl += 1; }
            0x3A => { self.set_a(bus.read(self.hl)); self.hl -= 1; }

            0x06 => { let d8 = self.fetch::<u8>(bus); self.set_b(d8);          }
            0x16 => { let d8 = self.fetch::<u8>(bus); self.set_d(d8);          }
            0x26 => { let d8 = self.fetch::<u8>(bus); self.set_d(d8);          }
            0x36 => { let d8 = self.fetch::<u8>(bus); bus.write(self.hl, d8); }
            0x0E => { let d8 = self.fetch::<u8>(bus); self.set_c(d8);          }
            0x1E => { let d8 = self.fetch::<u8>(bus); self.set_e(d8);          }
            0x2E => { let d8 = self.fetch::<u8>(bus); self.set_l(d8);          }
            0x3E => { let d8 = self.fetch::<u8>(bus); self.set_a(d8);          }

            0x40 => self.set_b(self.b()),
            0x41 => self.set_b(self.c()),
            0x42 => self.set_b(self.d()),
            0x43 => self.set_b(self.e()),
            0x44 => self.set_b(self.h()),
            0x45 => self.set_b(self.l()),
            0x46 => self.set_b(bus.read(self.hl)),
            0x47 => self.set_b(self.a()),
            0x48 => self.set_c(self.b()),
            0x49 => self.set_c(self.c()),
            0x4A => self.set_c(self.d()),
            0x4B => self.set_c(self.e()),
            0x4C => self.set_c(self.h()),
            0x4D => self.set_c(self.l()),
            0x4E => self.set_c(bus.read(self.hl)),
            0x4F => self.set_c(self.a()),
            0x50 => self.set_d(self.b()),
            0x51 => self.set_d(self.c()),
            0x52 => self.set_d(self.d()),
            0x53 => self.set_d(self.e()),
            0x54 => self.set_d(self.h()),
            0x55 => self.set_d(self.l()),
            0x56 => self.set_d(bus.read(self.hl)),
            0x57 => self.set_d(self.a()),
            0x58 => self.set_e(self.b()),
            0x59 => self.set_e(self.c()),
            0x5A => self.set_e(self.d()),
            0x5B => self.set_e(self.e()),
            0x5C => self.set_e(self.h()),
            0x5D => self.set_e(self.l()),
            0x5E => self.set_e(bus.read(self.hl)),
            0x5F => self.set_e(self.a()),
            0x60 => self.set_h(self.b()),
            0x61 => self.set_h(self.c()),
            0x62 => self.set_h(self.d()),
            0x63 => self.set_h(self.e()),
            0x64 => self.set_h(self.h()),
            0x65 => self.set_h(self.l()),
            0x66 => self.set_h(bus.read(self.hl)),
            0x67 => self.set_h(self.a()),
            0x68 => self.set_l(self.b()),
            0x69 => self.set_l(self.c()),
            0x6A => self.set_l(self.d()),
            0x6B => self.set_l(self.e()),
            0x6C => self.set_l(self.h()),
            0x6D => self.set_l(self.l()),
            0x6E => self.set_l(bus.read(self.hl)),
            0x6F => self.set_l(self.a()),
            0x78 => self.set_a(self.b()),
            0x79 => self.set_a(self.c()),
            0x7A => self.set_a(self.d()),
            0x7B => self.set_a(self.e()),
            0x7C => self.set_a(self.h()),
            0x7D => self.set_a(self.l()),
            0x7E => self.set_a(bus.read(self.hl)),
            0x7F => self.set_a(self.a()),

            0x70 => bus.write(self.hl, self.b()),
            0x71 => bus.write(self.hl, self.c()),
            0x72 => bus.write(self.hl, self.d()),
            0x73 => bus.write(self.hl, self.e()),
            0x74 => bus.write(self.hl, self.h()),
            0x75 => bus.write(self.hl, self.l()),
            0x77 => bus.write(self.hl, self.a()),

            0xE0 => { let d8 = u16::from(self.fetch::<u8>(bus)); bus.write(0xFF00 + d8, self.a()); }
            0xF0 => { let d8 = u16::from(self.fetch::<u8>(bus)); self.set_a(bus.read(0xFF00 + d8)); }

            0xE2 => bus.write(0xFF00 + u16::from(self.c()), self.a()),
            0xF2 => self.set_a(bus.read(0xFF00 + u16::from(self.c()))),

            0xEA => { let d16 = self.fetch::<u16>(bus); bus.write(d16, self.a()); }
            0xFA => { let d16 = self.fetch::<u16>(bus); self.set_a(bus.read(d16)); }

            /*
             * 16bit load/store/move instructions
             */
            0x01 => self.bc = self.fetch::<u16>(bus),
            0x11 => self.de = self.fetch::<u16>(bus),
            0x21 => self.hl = self.fetch::<u16>(bus),
            0x31 => self.sp = self.fetch::<u16>(bus),

            0xC1 => pop!(self, bus, true, bc),
            0xD1 => pop!(self, bus, true, de),
            0xE1 => pop!(self, bus, true, hl),
            0xF1 => pop!(self, bus, true, af),

            0xC5 => push!(self, bus, bc),
            0xD5 => push!(self, bus, de),
            0xE5 => push!(self, bus, hl),
            0xF5 => push!(self, bus, af),

            0x08 => { let a16 = self.fetch::<u16>(bus); bus.write(a16, self.sp); }
            0xF9 => self.sp = self.hl,

            0xF8 => unimplemented!(),

            /*
             * 8bit arithmetic/logical instructions
             */
            0x04 => { let v = inc!(self, self.b()); self.set_b(v); }
            0x14 => { let v = inc!(self, self.d()); self.set_d(v); }
            0x24 => { let v = inc!(self, self.h()); self.set_h(v); }
            0x0C => { let v = inc!(self, self.c()); self.set_c(v); }
            0x1C => { let v = inc!(self, self.e()); self.set_e(v); }
            0x2C => { let v = inc!(self, self.l()); self.set_l(v); }
            0x3C => { let v = inc!(self, self.a()); self.set_a(v); }
            0x34 => {
                let v = inc!(self, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0x05 => { let v = dec!(self, self.b()); self.set_b(v); }
            0x15 => { let v = dec!(self, self.d()); self.set_d(v); }
            0x25 => { let v = dec!(self, self.h()); self.set_h(v); }
            0x0D => { let v = dec!(self, self.c()); self.set_c(v); }
            0x1D => { let v = dec!(self, self.e()); self.set_e(v); }
            0x2D => { let v = dec!(self, self.l()); self.set_l(v); }
            0x3D => { let v = dec!(self, self.a()); self.set_a(v); }
            0x35 => {
                let v = dec!(self, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0x80 => add!(self, self.b()),
            0x81 => add!(self, self.c()),
            0x82 => add!(self, self.d()),
            0x83 => add!(self, self.e()),
            0x84 => add!(self, self.h()),
            0x85 => add!(self, self.l()),
            0x87 => add!(self, self.a()),
            0x86 => add!(self, (bus as &mut MemR<u8>).read(self.hl)),
            0xC6 => { let d8 = self.fetch::<u8>(bus); add!(self, d8); }

            0x88 => add!(self, self.b() + u8::from(self.cy())),
            0x89 => add!(self, self.c() + u8::from(self.cy())),
            0x8A => add!(self, self.d() + u8::from(self.cy())),
            0x8B => add!(self, self.e() + u8::from(self.cy())),
            0x8C => add!(self, self.h() + u8::from(self.cy())),
            0x8D => add!(self, self.l() + u8::from(self.cy())),
            0x8F => add!(self, self.a() + u8::from(self.cy())),
            0x8E => add!(self, (bus as &mut MemR<u8>).read(self.hl) + u8::from(self.cy())),
            0xCE => { let d8 = self.fetch::<u8>(bus); add!(self, d8 + u8::from(self.cy())); }

            0x90 => sub!(self, self.b()),
            0x91 => sub!(self, self.c()),
            0x92 => sub!(self, self.d()),
            0x93 => sub!(self, self.e()),
            0x94 => sub!(self, self.h()),
            0x95 => sub!(self, self.l()),
            0x97 => sub!(self, self.a()),
            0x96 => sub!(self, (bus as &mut MemR<u8>).read(self.hl)),
            0xD6 => { let d8 = self.fetch::<u8>(bus); sub!(self, d8); }

            0x98 => sub!(self, self.b() + u8::from(self.cy())),
            0x99 => sub!(self, self.c() + u8::from(self.cy())),
            0x9A => sub!(self, self.d() + u8::from(self.cy())),
            0x9B => sub!(self, self.e() + u8::from(self.cy())),
            0x9C => sub!(self, self.h() + u8::from(self.cy())),
            0x9D => sub!(self, self.l() + u8::from(self.cy())),
            0x9F => sub!(self, self.a() + u8::from(self.cy())),
            0x9E => sub!(self, (bus as &mut MemR<u8>).read(self.hl) + u8::from(self.cy())),
            0xDE => { let d8 = self.fetch::<u8>(bus); sub!(self, d8 + u8::from(self.cy())); }

            0xA0 => and!(self, self.b()),
            0xA1 => and!(self, self.c()),
            0xA2 => and!(self, self.d()),
            0xA3 => and!(self, self.e()),
            0xA4 => and!(self, self.h()),
            0xA5 => and!(self, self.l()),
            0xA7 => and!(self, self.a()),
            0xA6 => and!(self, (bus as &mut MemR<u8>).read(self.hl)),
            0xE6 => { let d8 = self.fetch::<u8>(bus); and!(self, d8); }

            0xA8 => xor!(self, self.b()),
            0xA9 => xor!(self, self.c()),
            0xAA => xor!(self, self.d()),
            0xAB => xor!(self, self.e()),
            0xAC => xor!(self, self.h()),
            0xAD => xor!(self, self.l()),
            0xAF => xor!(self, self.a()),
            0xAE => xor!(self, (bus as &mut MemR<u8>).read(self.hl)),
            0xEE => { let d8 = self.fetch::<u8>(bus); xor!(self, d8); }

            0xB0 => or!(self, self.b()),
            0xB1 => or!(self, self.c()),
            0xB2 => or!(self, self.d()),
            0xB3 => or!(self, self.e()),
            0xB4 => or!(self, self.h()),
            0xB5 => or!(self, self.l()),
            0xB7 => or!(self, self.a()),
            0xB6 => or!(self, (bus as &mut MemR<u8>).read(self.hl)),
            0xF6 => { let d8 = self.fetch::<u8>(bus); or!(self, d8); }

            0xB8 => cmp!(self, self.a(), self.b()),
            0xB9 => cmp!(self, self.a(), self.c()),
            0xBA => cmp!(self, self.a(), self.d()),
            0xBB => cmp!(self, self.a(), self.e()),
            0xBC => cmp!(self, self.a(), self.h()),
            0xBD => cmp!(self, self.a(), self.l()),
            0xBF => cmp!(self, self.a(), self.a()),
            0xBE => cmp!(self, self.a(), (bus as &mut MemR<u8>).read(self.hl)),
            0xFE => { let d8 = self.fetch::<u8>(bus); cmp!(self, self.a(), d8); }

            0x2F => { self.set_a(!self.a()); self.set_sf(true); self.set_hc(true); }
            0x37 => { self.set_sf(false); self.set_hc(false); self.set_cy(true); }
            0x3F => { self.set_sf(false); self.set_hc(false); self.set_cy(!self.cy()); }

            0x27 => unimplemented!(),

            /*
             * 	16bit arithmetic/logical instructions
             */
            0x03 => self.bc += 1,
            0x13 => self.de += 1,
            0x23 => self.hl += 1,
            0x33 => self.sp += 1,

            0x0B => self.bc -= 1,
            0x1B => self.de -= 1,
            0x2B => self.hl -= 1,
            0x3B => self.sp -= 1,

            0x09 => add16!(self, self.hl, self.bc),
            0x19 => add16!(self, self.hl, self.de),
            0x29 => add16!(self, self.hl, self.hl),
            0x39 => add16!(self, self.hl, self.sp),
            0xE8 => {
                let d8 = u16::from(self.fetch::<u8>(bus));
                add16!(self, self.sp, d8);
                self.set_zf(false);
            }

            /*
             * 8bit rotations/shifts and bit instructions
             */
            0x07 => { let v = rl!(self, true, self.a()); self.set_a(v); self.set_zf(false); }
            0x17 => { let v = rl!(self, false, self.a()); self.set_a(v); self.set_zf(false); }
            0x0F => { let v = rr!(self, true, self.a()); self.set_a(v); self.set_zf(false); }
            0x1F => { let v = rr!(self, false, self.a()); self.set_a(v); self.set_zf(false); }

            /*
             * Invalid opcodes
             */
            0xD3 | 0xDB | 0xDD | 0xE3 | 0xE4 | 0xEB | 0xEC | 0xED | 0xF4 | 0xFC | 0xFD => {
                panic!("unexpected opcode at {:04X}: 0x{:02X}", self.pc, opcode);
            }
        }
    }

    #[rustfmt::skip]
    #[allow(clippy::cyclomatic_complexity)]
    fn op_cb(&mut self, bus: &mut Bus, opcode: u8) {
        match opcode {
            0x00 => { let v = rl!(self, true, self.b()); self.set_b(v); }
            0x01 => { let v = rl!(self, true, self.c()); self.set_c(v); }
            0x02 => { let v = rl!(self, true, self.d()); self.set_d(v); }
            0x03 => { let v = rl!(self, true, self.e()); self.set_e(v); }
            0x04 => { let v = rl!(self, true, self.h()); self.set_h(v); }
            0x05 => { let v = rl!(self, true, self.l()); self.set_l(v); }
            0x07 => { let v = rl!(self, true, self.a()); self.set_a(v); }
            0x06 => {
                let v = rl!(self, true, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0x08 => { let v = rr!(self, true, self.b()); self.set_b(v); }
            0x09 => { let v = rr!(self, true, self.c()); self.set_c(v); }
            0x0A => { let v = rr!(self, true, self.d()); self.set_d(v); }
            0x0B => { let v = rr!(self, true, self.e()); self.set_e(v); }
            0x0C => { let v = rr!(self, true, self.h()); self.set_h(v); }
            0x0D => { let v = rr!(self, true, self.l()); self.set_l(v); }
            0x0F => { let v = rr!(self, true, self.a()); self.set_a(v); }
            0x0E => {
                let v = rr!(self, true, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0x10 => { let v = rl!(self, false, self.b()); self.set_b(v); }
            0x11 => { let v = rl!(self, false, self.c()); self.set_c(v); }
            0x12 => { let v = rl!(self, false, self.d()); self.set_d(v); }
            0x13 => { let v = rl!(self, false, self.e()); self.set_e(v); }
            0x14 => { let v = rl!(self, false, self.h()); self.set_h(v); }
            0x15 => { let v = rl!(self, false, self.l()); self.set_l(v); }
            0x17 => { let v = rl!(self, false, self.a()); self.set_a(v); }
            0x16 => {
                let v = rl!(self, false, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0x18 => { let v = rr!(self, false, self.b()); self.set_b(v); }
            0x19 => { let v = rr!(self, false, self.c()); self.set_c(v); }
            0x1A => { let v = rr!(self, false, self.d()); self.set_d(v); }
            0x1B => { let v = rr!(self, false, self.e()); self.set_e(v); }
            0x1C => { let v = rr!(self, false, self.h()); self.set_h(v); }
            0x1D => { let v = rr!(self, false, self.l()); self.set_l(v); }
            0x1F => { let v = rr!(self, false, self.a()); self.set_a(v); }
            0x1E => {
                let v = rr!(self, false, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0x40 => bit!(self, 0, self.b()),
            0x41 => bit!(self, 0, self.c()),
            0x42 => bit!(self, 0, self.d()),
            0x43 => bit!(self, 0, self.e()),
            0x44 => bit!(self, 0, self.h()),
            0x45 => bit!(self, 0, self.l()),
            0x47 => bit!(self, 0, self.a()),
            0x46 => bit!(self, 0, (bus as &mut MemR<u8>).read(self.hl)),

            0x48 => bit!(self, 1, self.b()),
            0x49 => bit!(self, 1, self.c()),
            0x4A => bit!(self, 1, self.d()),
            0x4B => bit!(self, 1, self.e()),
            0x4C => bit!(self, 1, self.h()),
            0x4D => bit!(self, 1, self.l()),
            0x4F => bit!(self, 1, self.a()),
            0x4E => bit!(self, 1, (bus as &mut MemR<u8>).read(self.hl)),

            0x50 => bit!(self, 2, self.b()),
            0x51 => bit!(self, 2, self.c()),
            0x52 => bit!(self, 2, self.d()),
            0x53 => bit!(self, 2, self.e()),
            0x54 => bit!(self, 2, self.h()),
            0x55 => bit!(self, 2, self.l()),
            0x57 => bit!(self, 2, self.a()),
            0x56 => bit!(self, 2, (bus as &mut MemR<u8>).read(self.hl)),

            0x58 => bit!(self, 3, self.b()),
            0x59 => bit!(self, 3, self.c()),
            0x5A => bit!(self, 3, self.d()),
            0x5B => bit!(self, 3, self.e()),
            0x5C => bit!(self, 3, self.h()),
            0x5D => bit!(self, 3, self.l()),
            0x5F => bit!(self, 3, self.a()),
            0x5E => bit!(self, 3, (bus as &mut MemR<u8>).read(self.hl)),

            0x60 => bit!(self, 4, self.b()),
            0x61 => bit!(self, 4, self.c()),
            0x62 => bit!(self, 4, self.d()),
            0x63 => bit!(self, 4, self.e()),
            0x64 => bit!(self, 4, self.h()),
            0x65 => bit!(self, 4, self.l()),
            0x67 => bit!(self, 4, self.a()),
            0x66 => bit!(self, 4, (bus as &mut MemR<u8>).read(self.hl)),

            0x68 => bit!(self, 5, self.b()),
            0x69 => bit!(self, 5, self.c()),
            0x6A => bit!(self, 5, self.d()),
            0x6B => bit!(self, 5, self.e()),
            0x6C => bit!(self, 5, self.h()),
            0x6D => bit!(self, 5, self.l()),
            0x6F => bit!(self, 5, self.a()),
            0x6E => bit!(self, 5, (bus as &mut MemR<u8>).read(self.hl)),

            0x70 => bit!(self, 6, self.b()),
            0x71 => bit!(self, 6, self.c()),
            0x72 => bit!(self, 6, self.d()),
            0x73 => bit!(self, 6, self.e()),
            0x74 => bit!(self, 6, self.h()),
            0x75 => bit!(self, 6, self.l()),
            0x77 => bit!(self, 6, self.a()),
            0x76 => bit!(self, 6, (bus as &mut MemR<u8>).read(self.hl)),

            0x78 => bit!(self, 7, self.b()),
            0x79 => bit!(self, 7, self.c()),
            0x7A => bit!(self, 7, self.d()),
            0x7B => bit!(self, 7, self.e()),
            0x7C => bit!(self, 7, self.h()),
            0x7D => bit!(self, 7, self.l()),
            0x7F => bit!(self, 7, self.a()),
            0x7E => bit!(self, 7, (bus as &mut MemR<u8>).read(self.hl)),

            0x80 => self.set_b(res!(0, self.b())),
            0x81 => self.set_c(res!(0, self.c())),
            0x82 => self.set_d(res!(0, self.d())),
            0x83 => self.set_e(res!(0, self.e())),
            0x84 => self.set_h(res!(0, self.h())),
            0x85 => self.set_l(res!(0, self.l())),
            0x87 => self.set_a(res!(0, self.a())),
            0x86 => {
                let v = res!(0, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0x88 => self.set_b(res!(1, self.b())),
            0x89 => self.set_c(res!(1, self.c())),
            0x8A => self.set_d(res!(1, self.d())),
            0x8B => self.set_e(res!(1, self.e())),
            0x8C => self.set_h(res!(1, self.h())),
            0x8D => self.set_l(res!(1, self.l())),
            0x8F => self.set_a(res!(1, self.a())),
            0x8E => {
                let v = res!(1, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0x90 => self.set_b(res!(2, self.b())),
            0x91 => self.set_c(res!(2, self.c())),
            0x92 => self.set_d(res!(2, self.d())),
            0x93 => self.set_e(res!(2, self.e())),
            0x94 => self.set_h(res!(2, self.h())),
            0x95 => self.set_l(res!(2, self.l())),
            0x97 => self.set_a(res!(2, self.a())),
            0x96 => {
                let v = res!(2, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0x98 => self.set_b(res!(3, self.b())),
            0x99 => self.set_c(res!(3, self.c())),
            0x9A => self.set_d(res!(3, self.d())),
            0x9B => self.set_e(res!(3, self.e())),
            0x9C => self.set_h(res!(3, self.h())),
            0x9D => self.set_l(res!(3, self.l())),
            0x9F => self.set_a(res!(3, self.a())),
            0x9E => {
                let v = res!(3, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0xA0 => self.set_b(res!(4, self.b())),
            0xA1 => self.set_c(res!(4, self.c())),
            0xA2 => self.set_d(res!(4, self.d())),
            0xA3 => self.set_e(res!(4, self.e())),
            0xA4 => self.set_h(res!(4, self.h())),
            0xA5 => self.set_l(res!(4, self.l())),
            0xA7 => self.set_a(res!(4, self.a())),
            0xA6 => {
                let v = res!(4, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0xA8 => self.set_b(res!(5, self.b())),
            0xA9 => self.set_c(res!(5, self.c())),
            0xAA => self.set_d(res!(5, self.d())),
            0xAB => self.set_e(res!(5, self.e())),
            0xAC => self.set_h(res!(5, self.h())),
            0xAD => self.set_l(res!(5, self.l())),
            0xAF => self.set_a(res!(5, self.a())),
            0xAE => {
                let v = res!(5, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0xB0 => self.set_b(res!(6, self.b())),
            0xB1 => self.set_c(res!(6, self.c())),
            0xB2 => self.set_d(res!(6, self.d())),
            0xB3 => self.set_e(res!(6, self.e())),
            0xB4 => self.set_h(res!(6, self.h())),
            0xB5 => self.set_l(res!(6, self.l())),
            0xB7 => self.set_a(res!(6, self.a())),
            0xB6 => {
                let v = res!(6, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0xB8 => self.set_b(res!(7, self.b())),
            0xB9 => self.set_c(res!(7, self.c())),
            0xBA => self.set_d(res!(7, self.d())),
            0xBB => self.set_e(res!(7, self.e())),
            0xBC => self.set_h(res!(7, self.h())),
            0xBD => self.set_l(res!(7, self.l())),
            0xBF => self.set_a(res!(7, self.a())),
            0xBE => {
                let v = res!(7, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0xC0 => self.set_b(set!(0, self.b())),
            0xC1 => self.set_c(set!(0, self.c())),
            0xC2 => self.set_d(set!(0, self.d())),
            0xC3 => self.set_e(set!(0, self.e())),
            0xC4 => self.set_h(set!(0, self.h())),
            0xC5 => self.set_l(set!(0, self.l())),
            0xC7 => self.set_a(set!(0, self.a())),
            0xC6 => {
                let v = set!(0, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0xC8 => self.set_b(set!(1, self.b())),
            0xC9 => self.set_c(set!(1, self.c())),
            0xCA => self.set_d(set!(1, self.d())),
            0xCB => self.set_e(set!(1, self.e())),
            0xCC => self.set_h(set!(1, self.h())),
            0xCD => self.set_l(set!(1, self.l())),
            0xCF => self.set_a(set!(1, self.a())),
            0xCE => {
                let v = set!(1, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0xD0 => self.set_b(set!(2, self.b())),
            0xD1 => self.set_c(set!(2, self.c())),
            0xD2 => self.set_d(set!(2, self.d())),
            0xD3 => self.set_e(set!(2, self.e())),
            0xD4 => self.set_h(set!(2, self.h())),
            0xD5 => self.set_l(set!(2, self.l())),
            0xD7 => self.set_a(set!(2, self.a())),
            0xD6 => {
                let v = set!(2, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0xD8 => self.set_b(set!(3, self.b())),
            0xD9 => self.set_c(set!(3, self.c())),
            0xDA => self.set_d(set!(3, self.d())),
            0xDB => self.set_e(set!(3, self.e())),
            0xDC => self.set_h(set!(3, self.h())),
            0xDD => self.set_l(set!(3, self.l())),
            0xDF => self.set_a(set!(3, self.a())),
            0xDE => {
                let v = set!(3, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0xE0 => self.set_b(set!(4, self.b())),
            0xE1 => self.set_c(set!(4, self.c())),
            0xE2 => self.set_d(set!(4, self.d())),
            0xE3 => self.set_e(set!(4, self.e())),
            0xE4 => self.set_h(set!(4, self.h())),
            0xE5 => self.set_l(set!(4, self.l())),
            0xE7 => self.set_a(set!(4, self.a())),
            0xE6 => {
                let v = set!(4, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0xE8 => self.set_b(set!(5, self.b())),
            0xE9 => self.set_c(set!(5, self.c())),
            0xEA => self.set_d(set!(5, self.d())),
            0xEB => self.set_e(set!(5, self.e())),
            0xEC => self.set_h(set!(5, self.h())),
            0xED => self.set_l(set!(5, self.l())),
            0xEF => self.set_a(set!(5, self.a())),
            0xEE => {
                let v = set!(5, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0xF0 => self.set_b(set!(6, self.b())),
            0xF1 => self.set_c(set!(6, self.c())),
            0xF2 => self.set_d(set!(6, self.d())),
            0xF3 => self.set_e(set!(6, self.e())),
            0xF4 => self.set_h(set!(6, self.h())),
            0xF5 => self.set_l(set!(6, self.l())),
            0xF7 => self.set_a(set!(6, self.a())),
            0xF6 => {
                let v = set!(6, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            0xF8 => self.set_b(set!(7, self.b())),
            0xF9 => self.set_c(set!(7, self.c())),
            0xFA => self.set_d(set!(7, self.d())),
            0xFB => self.set_e(set!(7, self.e())),
            0xFC => self.set_h(set!(7, self.h())),
            0xFD => self.set_l(set!(7, self.l())),
            0xFF => self.set_a(set!(7, self.a())),
            0xFE => {
                let v = set!(7, (bus as &mut MemR<u8>).read(self.hl));
                bus.write(self.hl, v);
            }

            _ => unimplemented!(),
        }
    }
}
