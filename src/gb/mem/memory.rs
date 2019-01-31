use super::dbg;
use super::{MemR, MemSize, MemW};

pub struct Memory {
    data: Vec<u8>,
}

impl Memory {
    pub fn new(size: u16) -> Memory {
        Memory {
            data: vec![0; usize::from(size)],
        }
    }
}

impl MemR for Memory {
    fn read<T: MemSize>(&self, addr: u16) -> Result<T, dbg::TraceEvent> {
        if usize::from(addr) < self.data.len() {
            Ok(T::read_le(&self.data[usize::from(addr)..]))
        } else {
            Err(dbg::TraceEvent::MemFault(addr))
        }
    }
}

impl MemW for Memory {
    fn write<T: MemSize>(&mut self, addr: u16, val: T) -> Result<(), dbg::TraceEvent> {
        if usize::from(addr) < self.data.len() {
            T::write_le(&mut self.data[usize::from(addr)..], val);
            Ok(())
        } else {
            Err(dbg::TraceEvent::MemFault(addr))
        }
    }
}
