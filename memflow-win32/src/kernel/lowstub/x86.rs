use crate::error::{Error, Result};
use crate::kernel::StartBlock;

use byteorder::{ByteOrder, LittleEndian};
use memflow_core::architecture::{self, Architecture};
use memflow_core::iter::PageChunks;
use memflow_core::types::Address;

fn check_page(base: Address, mem: &[u8]) -> bool {
    if mem[0] != 0x67 {
        return false;
    }

    if (LittleEndian::read_u32(&mem[0xc00..]) & 0xffff_f003) != (base.as_u32() + 0x3) {
        return false;
    }

    match mem
        .iter()
        .step_by(4)
        .skip(0x200)
        .filter(|&&x| x == 0x63 || x == 0xe3)
        .count()
    {
        x if x > 16 => true,
        _ => false,
    }
}

pub fn find(mem: &[u8]) -> Result<StartBlock> {
    mem.page_chunks(Address::from(0), architecture::x86::page_size())
        .find(|(a, c)| check_page(*a, c))
        .map(|(a, _)| StartBlock {
            arch: Architecture::X86,
            kernel_hint: 0.into(),
            dtb: a,
        })
        .ok_or_else(|| Error::Initialization("unable to find x86 dtb in lowstub < 16M"))
}