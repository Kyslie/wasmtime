
//! MIPS64 ABI implementation.

use super::registers::{FPR, GPR};
use super::settings;
use crate::abi;
use crate::ir::{self, Type};
use crate::isa::RegClass;
use crate::regalloc::RegisterSet;
use alloc::borrow::Cow;
use target_lexicon::Triple;

struct Args {
    pointer_bits: u8,
    pointer_bytes: u8,
    pointer_type: Type,
    regs: u32,
    reg_limit: u32,
    offset: u32,
}

impl Args {
    fn new(bits: u8, enable_e: bool) -> Self {
        Self {
            pointer_bits: bits,
            pointer_bytes: bits / 8,
            pointer_type: Type::int(u16::from(bits)).unwrap(),
            regs: 0,
            reg_limit: if enable_e { 6 } else { 8 },
            offset: 0,
        }
    }
}

/// Legalize 'sig' for MIPS64
pub fn legalize_signature(
    sig: &mut Cow<ir::Signature>,
    triple: &Triple,
    isa_flags: &settings::Flags,
    current: bool,
) {
    unimplemented!()
}

/// Get register class for a type appearing in a legalized signature.
pub fn regclass_for_abi_type(ty: Type) -> RegClass {
    if ty.is_float() {
        FPR
    } else {
        GPR
    }
}

pub fn allocatable_registers(_func: &ir::Function, isa_flags: &settings::Flags) -> RegisterSet {
    // Todo
    let mut regs = RegisterSet::new();
    regs.take(GPR, GPR.unit(0)); // Hard-wired 0.
    regs.take(GPR, GPR.unit(28)); // global pointer
    regs.take(GPR, GPR.unit(29)); // stack pointer
    regs.take(GPR, GPR.unit(31)); // return address

    regs
}
