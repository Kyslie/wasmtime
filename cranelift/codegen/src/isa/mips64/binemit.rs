//! Emitting binary MIPS64 machine code.

use crate::binemit::{bad_encoding, CodeSink, Reloc};
use crate::ir::{Function, Inst, InstructionData};
use crate::isa::{RegUnit, StackBaseMask, StackRef, TargetIsa};
use crate::regalloc::RegDiversions;
use core::u32;

include!(concat!(env!("OUT_DIR"), "/binemit-mips64.rs"));

/// R-type instructions.
fn put_r<CS: CodeSink + ?Sized>(bits: u16, rs: RegUnit, rt: RegUnit, rd: RegUnit, sink: &mut CS) {
    let bits = u32::from(bits);
    let opcode6 = (bits >> 11) & 0x3f;
    let shamt5 = (bits >> 6) & 0x1f;
    let funct6 = bits & 0x3f;
    let rs = u32::from(rs) & 0x1f;
    let rt = u32::from(rt) & 0x1f;
    let rd = u32::from(rd) & 0x1f;

    let mut i = 0x0;
    i |= funct6;
    i |= shamt5 << 6;
    i |= rd << 11;
    i |= rt << 16;
    i |= rs << 21;
    i |= opcode6 << 26;

    sink.put4(i);
}
