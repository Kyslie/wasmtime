#![allow(dead_code)] // for now

use crate::microwasm::{BrTarget, SignlessType, Type, F32, F64, I32, I64};

use self::registers::*;
use crate::error::Error;
use crate::microwasm::Value;
use crate::module::{ModuleContext, RuntimeFunc};
use cranelift_codegen::binemit;
use dynasmrt::x64::Assembler;
use dynasmrt::{AssemblyOffset, DynamicLabel, DynasmApi, DynasmLabelApi, ExecutableBuffer};
use std::{
    iter::{self, FromIterator},
    mem,
    ops::RangeInclusive,
};

// TODO: Get rid of this! It's a total hack.
mod magic {
    use cranelift_codegen::ir;

    /// Compute an `ir::ExternalName` for the `memory.grow` libcall for
    /// 32-bit locally-defined memories.
    pub fn get_memory32_grow_name() -> ir::ExternalName {
        ir::ExternalName::user(1, 0)
    }

    /// Compute an `ir::ExternalName` for the `memory.grow` libcall for
    /// 32-bit imported memories.
    pub fn get_imported_memory32_grow_name() -> ir::ExternalName {
        ir::ExternalName::user(1, 1)
    }

    /// Compute an `ir::ExternalName` for the `memory.size` libcall for
    /// 32-bit locally-defined memories.
    pub fn get_memory32_size_name() -> ir::ExternalName {
        ir::ExternalName::user(1, 2)
    }

    /// Compute an `ir::ExternalName` for the `memory.size` libcall for
    /// 32-bit imported memories.
    pub fn get_imported_memory32_size_name() -> ir::ExternalName {
        ir::ExternalName::user(1, 3)
    }
}

/// Size of a pointer on the target in bytes.
const WORD_SIZE: u32 = 8;

type RegId = u8;

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub enum GPR {
    Rq(RegId),
    Rx(RegId),
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub enum GPRType {
    Rq,
    Rx,
}

impl From<SignlessType> for GPRType {
    fn from(other: SignlessType) -> GPRType {
        match other {
            I32 | I64 => GPRType::Rq,
            F32 | F64 => GPRType::Rx,
        }
    }
}

impl From<SignlessType> for Option<GPRType> {
    fn from(other: SignlessType) -> Self {
        Some(other.into())
    }
}

impl GPR {
    fn type_(self) -> GPRType {
        match self {
            GPR::Rq(_) => GPRType::Rq,
            GPR::Rx(_) => GPRType::Rx,
        }
    }

    fn rq(self) -> Option<RegId> {
        match self {
            GPR::Rq(r) => Some(r),
            GPR::Rx(_) => None,
        }
    }

    fn rx(self) -> Option<RegId> {
        match self {
            GPR::Rx(r) => Some(r),
            GPR::Rq(_) => None,
        }
    }
}

pub fn arg_locs(types: impl IntoIterator<Item = SignlessType>) -> Vec<CCLoc> {
    let types = types.into_iter();
    let mut out = Vec::with_capacity(types.size_hint().0);
    // TODO: VmCtx is in the first register
    let mut int_gpr_iter = INTEGER_ARGS_IN_GPRS.iter();
    let mut float_gpr_iter = FLOAT_ARGS_IN_GPRS.iter();
    let mut stack_idx = 0;

    for ty in types {
        match ty {
            I32 | I64 => out.push(int_gpr_iter.next().map(|&r| CCLoc::Reg(r)).unwrap_or_else(
                || {
                    let out = CCLoc::Stack(stack_idx);
                    stack_idx += 1;
                    out
                },
            )),
            F32 | F64 => out.push(
                float_gpr_iter
                    .next()
                    .map(|&r| CCLoc::Reg(r))
                    .expect("Float args on stack not yet supported"),
            ),
        }
    }

    out
}

pub fn ret_locs(types: impl IntoIterator<Item = SignlessType>) -> Vec<CCLoc> {
    let types = types.into_iter();
    let mut out = Vec::with_capacity(types.size_hint().0);
    // TODO: VmCtx is in the first register
    let mut int_gpr_iter = INTEGER_RETURN_GPRS.iter();
    let mut float_gpr_iter = FLOAT_RETURN_GPRS.iter();

    for ty in types {
        match ty {
            I32 | I64 => out.push(CCLoc::Reg(
                *int_gpr_iter
                    .next()
                    .expect("We don't support stack returns yet"),
            )),
            F32 | F64 => out.push(CCLoc::Reg(
                *float_gpr_iter
                    .next()
                    .expect("We don't support stack returns yet"),
            )),
        }
    }

    out
}

#[derive(Debug, Copy, Clone)]
struct GPRs {
    bits: u16,
}

impl GPRs {
    fn new() -> Self {
        Self { bits: 0 }
    }
}

pub mod registers {
    use super::{RegId, GPR};

    pub mod rq {
        use super::RegId;

        pub const RAX: RegId = 0;
        pub const RCX: RegId = 1;
        pub const RDX: RegId = 2;
        pub const RBX: RegId = 3;
        pub const RSP: RegId = 4;
        pub const RBP: RegId = 5;
        pub const RSI: RegId = 6;
        pub const RDI: RegId = 7;
        pub const R8: RegId = 8;
        pub const R9: RegId = 9;
        pub const R10: RegId = 10;
        pub const R11: RegId = 11;
        pub const R12: RegId = 12;
        pub const R13: RegId = 13;
        pub const R14: RegId = 14;
        pub const R15: RegId = 15;
    }

    pub const RAX: GPR = GPR::Rq(self::rq::RAX);
    pub const RCX: GPR = GPR::Rq(self::rq::RCX);
    pub const RDX: GPR = GPR::Rq(self::rq::RDX);
    pub const RBX: GPR = GPR::Rq(self::rq::RBX);
    pub const RSP: GPR = GPR::Rq(self::rq::RSP);
    pub const RBP: GPR = GPR::Rq(self::rq::RBP);
    pub const RSI: GPR = GPR::Rq(self::rq::RSI);
    pub const RDI: GPR = GPR::Rq(self::rq::RDI);
    pub const R8: GPR = GPR::Rq(self::rq::R8);
    pub const R9: GPR = GPR::Rq(self::rq::R9);
    pub const R10: GPR = GPR::Rq(self::rq::R10);
    pub const R11: GPR = GPR::Rq(self::rq::R11);
    pub const R12: GPR = GPR::Rq(self::rq::R12);
    pub const R13: GPR = GPR::Rq(self::rq::R13);
    pub const R14: GPR = GPR::Rq(self::rq::R14);
    pub const R15: GPR = GPR::Rq(self::rq::R15);

    pub const XMM0: GPR = GPR::Rx(0);
    pub const XMM1: GPR = GPR::Rx(1);
    pub const XMM2: GPR = GPR::Rx(2);
    pub const XMM3: GPR = GPR::Rx(3);
    pub const XMM4: GPR = GPR::Rx(4);
    pub const XMM5: GPR = GPR::Rx(5);
    pub const XMM6: GPR = GPR::Rx(6);
    pub const XMM7: GPR = GPR::Rx(7);
    pub const XMM8: GPR = GPR::Rx(8);
    pub const XMM9: GPR = GPR::Rx(9);
    pub const XMM10: GPR = GPR::Rx(10);
    pub const XMM11: GPR = GPR::Rx(11);
    pub const XMM12: GPR = GPR::Rx(12);
    pub const XMM13: GPR = GPR::Rx(13);
    pub const XMM14: GPR = GPR::Rx(14);
    pub const XMM15: GPR = GPR::Rx(15);

    pub const NUM_GPRS: u8 = 16;
}

extern "sysv64" fn println(len: u64, args: *const u8) {
    println!("{}", unsafe {
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(args, len as usize))
    });
}

#[allow(unused_macros)]
macro_rules! asm_println {
    ($asm:expr) => {asm_println!($asm,)};
    ($asm:expr, $($args:tt)*) => {{
        use std::mem;

        let mut args = format!($($args)*).into_bytes();

        let len = args.len();
        let ptr = args.as_mut_ptr();
        mem::forget(args);

        dynasm!($asm
            ; push rdi
            ; push rsi
            ; push rdx
            ; push rcx
            ; push r8
            ; push r9
            ; push r10
            ; push r11

            ; mov rax, QWORD println as *const u8 as i64
            ; mov rdi, QWORD len as i64
            ; mov rsi, QWORD ptr as i64

            ; test rsp, 0b1111
            ; jnz >with_adjusted_stack_ptr

            ; call rax
            ; jmp >pop_rest

            ; with_adjusted_stack_ptr:
            ; push 1
            ; call rax
            ; pop r11

            ; pop_rest:
            ; pop r11
            ; pop r10
            ; pop r9
            ; pop r8
            ; pop rcx
            ; pop rdx
            ; pop rsi
            ; pop rdi
        );
    }}
}

impl GPRs {
    fn take(&mut self) -> RegId {
        let lz = self.bits.trailing_zeros();
        debug_assert!(lz < 16, "ran out of free GPRs");
        let gpr = lz as RegId;
        self.mark_used(gpr);
        gpr
    }

    fn mark_used(&mut self, gpr: RegId) {
        self.bits &= !(1 << gpr as u16);
    }

    fn release(&mut self, gpr: RegId) {
        debug_assert!(
            !self.is_free(gpr),
            "released register {} was already free",
            gpr
        );
        self.bits |= 1 << gpr;
    }

    fn free_count(&self) -> u32 {
        self.bits.count_ones()
    }

    fn is_free(&self, gpr: RegId) -> bool {
        (self.bits & (1 << gpr)) != 0
    }
}

#[derive(Debug, Copy, Clone)]
pub struct Registers {
    /// Registers at 64 bits and below (al/ah/ax/eax/rax, for example)
    scratch_64: (GPRs, [u8; NUM_GPRS as usize]),
    /// Registers at 128 bits (xmm0, for example)
    scratch_128: (GPRs, [u8; NUM_GPRS as usize]),
}

impl Default for Registers {
    fn default() -> Self {
        Self::new()
    }
}

impl Registers {
    pub fn new() -> Self {
        let mut result = Self {
            scratch_64: (GPRs::new(), [1; NUM_GPRS as _]),
            scratch_128: (GPRs::new(), [1; NUM_GPRS as _]),
        };

        // Give ourselves a few scratch registers to work with, for now.
        for &scratch in SCRATCH_REGS {
            result.release(scratch);
        }

        result
    }

    fn scratch_counts_mut(&mut self, gpr: GPR) -> (u8, &mut (GPRs, [u8; NUM_GPRS as usize])) {
        match gpr {
            GPR::Rq(r) => (r, &mut self.scratch_64),
            GPR::Rx(r) => (r, &mut self.scratch_128),
        }
    }

    fn scratch_counts(&self, gpr: GPR) -> (u8, &(GPRs, [u8; NUM_GPRS as usize])) {
        match gpr {
            GPR::Rq(r) => (r, &self.scratch_64),
            GPR::Rx(r) => (r, &self.scratch_128),
        }
    }

    pub fn mark_used(&mut self, gpr: GPR) {
        let (gpr, scratch_counts) = self.scratch_counts_mut(gpr);
        scratch_counts.0.mark_used(gpr);
        scratch_counts.1[gpr as usize] += 1;
    }

    pub fn num_usages(&self, gpr: GPR) -> u8 {
        let (gpr, scratch_counts) = self.scratch_counts(gpr);
        scratch_counts.1[gpr as usize]
    }

    pub fn take(&mut self, ty: impl Into<GPRType>) -> GPR {
        let (mk_gpr, scratch_counts) = match ty.into() {
            GPRType::Rq => (GPR::Rq as fn(_) -> _, &mut self.scratch_64),
            GPRType::Rx => (GPR::Rx as fn(_) -> _, &mut self.scratch_128),
        };

        let out = scratch_counts.0.take();
        scratch_counts.1[out as usize] += 1;
        mk_gpr(out)
    }

    pub fn release(&mut self, gpr: GPR) {
        let (gpr, scratch_counts) = self.scratch_counts_mut(gpr);
        let c = &mut scratch_counts.1[gpr as usize];
        *c -= 1;
        if *c == 0 {
            scratch_counts.0.release(gpr);
        }
    }

    pub fn is_free(&self, gpr: GPR) -> bool {
        let (gpr, scratch_counts) = self.scratch_counts(gpr);
        scratch_counts.0.is_free(gpr)
    }

    pub fn free_64(&self) -> u32 {
        self.scratch_64.0.free_count()
    }

    pub fn free_128(&self) -> u32 {
        self.scratch_128.0.free_count()
    }
}

#[derive(Debug, Clone)]
pub struct CallingConvention {
    pub stack_depth: StackDepth,
    pub arguments: Vec<CCLoc>,
}

impl CallingConvention {
    pub fn function_start(args: impl IntoIterator<Item = CCLoc>) -> Self {
        CallingConvention {
            // We start and return the function with stack depth 1 since we must
            // allow space for the saved return address.
            stack_depth: StackDepth(1),
            arguments: Vec::from_iter(args),
        }
    }
}

// TODO: Combine this with `ValueLocation`?
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CCLoc {
    /// Value exists in a register.
    Reg(GPR),
    /// Value exists on the stack.
    Stack(i32),
}

// TODO: Allow pushing condition codes to stack? We'd have to immediately
//       materialise them into a register if anything is pushed above them.
/// Describes location of a value.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ValueLocation {
    /// Value exists in a register.
    Reg(GPR),
    /// Value exists on the stack. Note that this offset is from the rsp as it
    /// was when we entered the function.
    Stack(i32),
    /// Value is a literal
    Immediate(Value),
}

impl From<CCLoc> for ValueLocation {
    fn from(other: CCLoc) -> Self {
        match other {
            CCLoc::Reg(r) => ValueLocation::Reg(r),
            CCLoc::Stack(o) => ValueLocation::Stack(o),
        }
    }
}

impl ValueLocation {
    fn immediate(self) -> Option<Value> {
        match self {
            ValueLocation::Immediate(i) => Some(i),
            _ => None,
        }
    }

    fn imm_i32(self) -> Option<i32> {
        self.immediate().and_then(Value::as_i32)
    }

    fn imm_i64(self) -> Option<i64> {
        self.immediate().and_then(Value::as_i64)
    }

    fn imm_f32(self) -> Option<wasmparser::Ieee32> {
        self.immediate().and_then(Value::as_f32)
    }

    fn imm_f64(self) -> Option<wasmparser::Ieee64> {
        self.immediate().and_then(Value::as_f64)
    }
}

// TODO: This assumes only system-v calling convention.
// In system-v calling convention the first 6 arguments are passed via registers.
// All rest arguments are passed on the stack.
const INTEGER_ARGS_IN_GPRS: &[GPR] = &[RSI, RDX, RCX, R8, R9];
const INTEGER_RETURN_GPRS: &[GPR] = &[RAX, RDX];
const FLOAT_ARGS_IN_GPRS: &[GPR] = &[XMM0, XMM1, XMM2, XMM3, XMM4, XMM5, XMM6, XMM7];
const FLOAT_RETURN_GPRS: &[GPR] = &[XMM0, XMM1];
// List of scratch registers taken from https://wiki.osdev.org/System_V_ABI
const SCRATCH_REGS: &[GPR] = &[
    RSI, RDX, RCX, R8, R9, RAX, R10, R11, XMM0, XMM1, XMM2, XMM3, XMM4, XMM5, XMM6, XMM7, XMM8,
    XMM9, XMM10, XMM11, XMM12, XMM13, XMM14, XMM15,
];
const VMCTX: RegId = rq::RDI;

#[must_use]
#[derive(Debug, Clone)]
pub struct FunctionEnd {
    should_generate_epilogue: bool,
}

pub struct CodeGenSession<'a, M> {
    assembler: Assembler,
    pub module_context: &'a M,
    labels: Labels,
    func_starts: Vec<(Option<AssemblyOffset>, DynamicLabel)>,
}

impl<'a, M> CodeGenSession<'a, M> {
    pub fn new(func_count: u32, module_context: &'a M) -> Self {
        let mut assembler = Assembler::new().unwrap();
        let func_starts = iter::repeat_with(|| (None, assembler.new_dynamic_label()))
            .take(func_count as usize)
            .collect::<Vec<_>>();

        CodeGenSession {
            assembler,
            labels: Default::default(),
            func_starts,
            module_context,
        }
    }

    pub fn new_context<'this>(
        &'this mut self,
        func_idx: u32,
        reloc_sink: &'this mut dyn binemit::RelocSink,
    ) -> Context<'this, M> {
        {
            let func_start = &mut self.func_starts[func_idx as usize];

            // At this point we know the exact start address of this function. Save it
            // and define dynamic label at this location.
            func_start.0 = Some(self.assembler.offset());
            self.assembler.dynamic_label(func_start.1);
        }

        Context {
            asm: &mut self.assembler,
            current_function: func_idx,
            reloc_sink: reloc_sink,
            func_starts: &self.func_starts,
            labels: &mut self.labels,
            block_state: Default::default(),
            module_context: self.module_context,
        }
    }

    pub fn into_translated_code_section(self) -> Result<TranslatedCodeSection, Error> {
        let exec_buf = self
            .assembler
            .finalize()
            .map_err(|_asm| Error::Assembler("assembler error".to_owned()))?;
        let func_starts = self
            .func_starts
            .iter()
            .map(|(offset, _)| offset.unwrap())
            .collect::<Vec<_>>();
        Ok(TranslatedCodeSection {
            exec_buf,
            func_starts,
            // TODO
            relocatable_accesses: vec![],
        })
    }
}

#[derive(Debug)]
struct RelocateAddress {
    reg: Option<GPR>,
    imm: usize,
}

#[derive(Debug)]
struct RelocateAccess {
    position: AssemblyOffset,
    dst_reg: GPR,
    address: RelocateAddress,
}

#[derive(Debug)]
pub struct UninitializedCodeSection(TranslatedCodeSection);

#[derive(Debug)]
pub struct TranslatedCodeSection {
    exec_buf: ExecutableBuffer,
    func_starts: Vec<AssemblyOffset>,
    relocatable_accesses: Vec<RelocateAccess>,
}

impl TranslatedCodeSection {
    pub fn func_start(&self, idx: usize) -> *const u8 {
        let offset = self.func_starts[idx];
        self.exec_buf.ptr(offset)
    }

    pub fn func_range(&self, idx: usize) -> std::ops::Range<usize> {
        let end = self
            .func_starts
            .get(idx + 1)
            .map(|i| i.0)
            .unwrap_or(self.exec_buf.len());

        self.func_starts[idx].0..end
    }

    pub fn funcs<'a>(&'a self) -> impl Iterator<Item = std::ops::Range<usize>> + 'a {
        (0..self.func_starts.len()).map(move |i| self.func_range(i))
    }

    pub fn buffer(&self) -> &[u8] {
        &*self.exec_buf
    }

    pub fn disassemble(&self) {
        crate::disassemble::disassemble(&*self.exec_buf).unwrap();
    }
}

/// A value on the logical stack. The logical stack is the value stack as it
/// is visible to the WebAssembly, whereas the physical stack is the stack as
/// it exists on the machine (i.e. as offsets in memory relative to `rsp`).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum StackValue {
    /// This value has a "real" location, either in a register, on the stack,
    /// in an immediate, etc.
    Value(ValueLocation),
    /// This value is on the physical stack and so should be accessed
    /// with the `pop` instruction.
    // TODO: This complicates a lot of our code, it'd be great if we could get rid of it.
    Pop,
}

impl StackValue {
    /// Returns either the location that this value can be accessed at
    /// if possible. If this value is `Pop`, you can only access it by
    /// popping the physical stack and so this function returns `None`.
    ///
    /// Of course, we could calculate the location of the value on the
    /// physical stack, but that would be unncessary computation for
    /// our usecases.
    fn location(&self) -> Option<ValueLocation> {
        match *self {
            StackValue::Value(loc) => Some(loc),
            StackValue::Pop => None,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct BlockState {
    stack: Stack,
    depth: StackDepth,
    regs: Registers,
}

type Stack = Vec<ValueLocation>;

pub enum MemoryAccessMode {
    /// This is slower than using `Unchecked` mode, but works in
    /// any scenario (the most important scenario being when we're
    /// running on a system that can't index much more memory than
    /// the Wasm).
    Checked,
    /// This means that checks are _not emitted by the compiler_!
    /// If you're using WebAssembly to run untrusted code, you
    /// _must_ delegate bounds checking somehow (probably by
    /// allocating 2^33 bytes of memory with the second half set
    /// to unreadable/unwriteable/unexecutable)
    Unchecked,
}

struct PendingLabel {
    label: Label,
    is_defined: bool,
}

impl PendingLabel {
    fn undefined(label: Label) -> Self {
        PendingLabel {
            label,
            is_defined: false,
        }
    }

    fn defined(label: Label) -> Self {
        PendingLabel {
            label,
            is_defined: true,
        }
    }

    fn as_undefined(&self) -> Option<Label> {
        if !self.is_defined {
            Some(self.label)
        } else {
            None
        }
    }
}

impl From<Label> for PendingLabel {
    fn from(label: Label) -> Self {
        PendingLabel {
            label,
            is_defined: false,
        }
    }
}

// TODO: We can share one trap/constant for all functions by reusing this struct
#[derive(Default)]
struct Labels {
    trap: Option<PendingLabel>,
    ret: Option<PendingLabel>,
    neg_const_f32: Option<PendingLabel>,
    neg_const_f64: Option<PendingLabel>,
    abs_const_f32: Option<PendingLabel>,
    abs_const_f64: Option<PendingLabel>,
}

pub struct Context<'a, M> {
    asm: &'a mut Assembler,
    reloc_sink: &'a mut dyn binemit::RelocSink,
    module_context: &'a M,
    current_function: u32,
    func_starts: &'a Vec<(Option<AssemblyOffset>, DynamicLabel)>,
    /// Each push and pop on the value stack increments or decrements this value by 1 respectively.
    pub block_state: BlockState,
    labels: &'a mut Labels,
}

/// Label in code.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Label(DynamicLabel);

/// Offset from starting value of SP counted in words.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub struct StackDepth(u32);

impl StackDepth {
    pub fn reserve(&mut self, slots: u32) {
        self.0 += slots;
    }

    pub fn free(&mut self, slots: u32) {
        self.0 -= slots;
    }
}

macro_rules! int_div {
    ($full_div_s:ident, $full_div_u:ident, $div_u:ident, $div_s:ident, $rem_u:ident, $rem_s:ident, $imm_fn:ident, $signed_ty:ty, $unsigned_ty:ty) => {
        // TODO: Fast div using mul for constant divisor? It looks like LLVM doesn't do that for us when
        //       emitting Wasm.
        pub fn $div_u(&mut self) {
            let divisor = self.pop();
            let quotient = self.pop();

            if let (Some(quotient), Some(divisor)) = (quotient.$imm_fn(), divisor.$imm_fn()) {
                if divisor == 0 {
                    self.trap();
                    self.push(ValueLocation::Immediate((0 as $unsigned_ty).into()));
                } else {
                    self.push(ValueLocation::Immediate(
                        <$unsigned_ty>::wrapping_div(quotient as _, divisor as _).into(),
                    ));
                }

                return;
            }

            let (div, rem, saved) = self.$full_div_u(divisor, quotient);

            self.free_value(rem);

            let div = match div {
                ValueLocation::Reg(div) if saved.clone().any(|(_, dst)| dst == div) => {
                    let new = self.block_state.regs.take(I32);
                    dynasm!(self.asm
                        ; mov Rq(new.rq().unwrap()), Rq(div.rq().unwrap())
                    );
                    self.block_state.regs.release(div);
                    ValueLocation::Reg(new)
                }
                _ => div,
            };

            self.cleanup_gprs(saved);

            self.push(div);
        }

        // TODO: Fast div using mul for constant divisor? It looks like LLVM doesn't do that for us when
        //       emitting Wasm.
        pub fn $div_s(&mut self) {
            let divisor = self.pop();
            let quotient = self.pop();

            if let (Some(quotient), Some(divisor)) = (quotient.$imm_fn(), divisor.$imm_fn()) {
                if divisor == 0 {
                    self.trap();
                    self.push(ValueLocation::Immediate((0 as $signed_ty).into()));
                } else {
                    self.push(ValueLocation::Immediate(
                        <$signed_ty>::wrapping_div(quotient, divisor).into(),
                    ));
                }

                return;
            }

            let (div, rem, saved) = self.$full_div_s(divisor, quotient);

            self.free_value(rem);

            let div = match div {
                ValueLocation::Reg(div) if saved.clone().any(|(_, dst)| dst == div) => {
                    let new = self.block_state.regs.take(I32);
                    dynasm!(self.asm
                        ; mov Rq(new.rq().unwrap()), Rq(div.rq().unwrap())
                    );
                    self.block_state.regs.release(div);
                    ValueLocation::Reg(new)
                }
                _ => div,
            };

            self.cleanup_gprs(saved);

            self.push(div);
        }

        pub fn $rem_u(&mut self) {
            let divisor = self.pop();
            let quotient = self.pop();

            if let (Some(quotient), Some(divisor)) = (quotient.$imm_fn(), divisor.$imm_fn()) {
                if divisor == 0 {
                    self.trap();
                    self.push(ValueLocation::Immediate((0 as $unsigned_ty).into()));
                } else {
                    self.push(ValueLocation::Immediate(
                        (quotient as $unsigned_ty % divisor as $unsigned_ty).into(),
                    ));
                }
                return;
            }

            let (div, rem, saved) = self.$full_div_u(divisor, quotient);

            self.free_value(div);

            let rem = match rem {
                ValueLocation::Reg(rem) if saved.clone().any(|(_, dst)| dst == rem) => {
                    let new = self.block_state.regs.take(I32);
                    dynasm!(self.asm
                        ; mov Rq(new.rq().unwrap()), Rq(rem.rq().unwrap())
                    );
                    self.block_state.regs.release(rem);
                    ValueLocation::Reg(new)
                }
                _ => rem,
            };

            self.cleanup_gprs(saved);

            self.push(rem);
        }

        pub fn $rem_s(&mut self) {
            let divisor = self.pop();
            let quotient = self.pop();

            if let (Some(quotient), Some(divisor)) = (quotient.$imm_fn(), divisor.$imm_fn()) {
                if divisor == 0 {
                    self.trap();
                    self.push(ValueLocation::Immediate((0 as $signed_ty).into()));
                } else {
                    self.push(ValueLocation::Immediate((quotient % divisor).into()));
                }
                return;
            }

            let (div, rem, saved) = self.$full_div_s(divisor, quotient);

            self.free_value(div);

            let rem = match rem {
                ValueLocation::Reg(rem) if saved.clone().any(|(_, dst)| dst == rem) => {
                    let new = self.block_state.regs.take(I32);
                    dynasm!(self.asm
                        ; mov Rq(new.rq().unwrap()), Rq(rem.rq().unwrap())
                    );
                    self.block_state.regs.release(rem);
                    ValueLocation::Reg(new)
                }
                _ => rem,
            };

            self.cleanup_gprs(saved);

            self.push(rem);
        }
    }
}

macro_rules! unop {
    ($name:ident, $instr:ident, $reg_ty:ident, $typ:ty, $const_fallback:expr) => {
        pub fn $name(&mut self) {
            let val = self.pop();

            let out_val = match val {
                ValueLocation::Immediate(imm) =>
                    ValueLocation::Immediate(
                        ($const_fallback(imm.as_int().unwrap() as $typ) as $typ).into()
                    ),
                ValueLocation::Stack(offset) => {
                    let offset = self.adjusted_offset(offset);
                    let temp = self.block_state.regs.take(Type::for_::<$typ>());
                    dynasm!(self.asm
                        ; $instr $reg_ty(temp.rq().unwrap()), [rsp + offset]
                    );
                    ValueLocation::Reg(temp)
                }
                ValueLocation::Reg(reg) => {
                    let temp = self.block_state.regs.take(Type::for_::<$typ>());
                    dynasm!(self.asm
                        ; $instr $reg_ty(temp.rq().unwrap()), $reg_ty(reg.rq().unwrap())
                    );
                    ValueLocation::Reg(temp)
                }
            };

            self.free_value(val);
            self.push(out_val);
        }
    }
}

macro_rules! conversion {
    (
        $name:ident,
        $instr:ident,
        $in_reg_ty:ident,
        $in_reg_fn:ident,
        $out_reg_ty:ident,
        $out_reg_fn:ident,
        $in_typ:ty,
        $out_typ:ty,
        $const_ty_fn:ident,
        $const_fallback:expr
    ) => {
        pub fn $name(&mut self) {
            let mut val = self.pop();

            let out_val = match val {
                ValueLocation::Immediate(imm) =>
                    ValueLocation::Immediate(
                        $const_fallback(imm.$const_ty_fn().unwrap()).into()
                    ),
                ValueLocation::Stack(offset) => {
                    let offset = self.adjusted_offset(offset);
                    let temp = self.block_state.regs.take(Type::for_::<$out_typ>());
                    dynasm!(self.asm
                        ; $instr $out_reg_ty(temp.$out_reg_fn().unwrap()), DWORD [rsp + offset]
                    );
                    ValueLocation::Reg(temp)
                }
                ValueLocation::Reg(_) => {
                    let reg = self.into_reg(Type::for_::<$in_typ>(), val);
                    let temp = self.block_state.regs.take(Type::for_::<$out_typ>());
                    val = ValueLocation::Reg(reg);

                    dynasm!(self.asm
                        ; $instr $out_reg_ty(temp.$out_reg_fn().unwrap()), $in_reg_ty(reg.$in_reg_fn().unwrap())
                    );
                    ValueLocation::Reg(temp)
                }
            };

            self.free_value(val);

            self.push(out_val);
        }
    }
}

// TODO: Support immediate `count` parameters
macro_rules! shift {
    ($name:ident, $reg_ty:ident, $instr:ident, $const_fallback:expr, $ty:expr) => {
        pub fn $name(&mut self) {
            enum RestoreRcx {
                MoveValBack(GPR),
                PopRcx,
            }

            let mut count = self.pop();
            let mut val = self.pop();

            if val == ValueLocation::Reg(RCX) {
                val = ValueLocation::Reg(self.into_temp_reg($ty, val));
            }

            // TODO: Maybe allocate `RCX`, write `count` to it and then free `count`.
            //       Once we've implemented refcounting this will do the right thing
            //       for free.
            let temp_rcx = match count {
                ValueLocation::Reg(RCX) => {None}
                other => {
                    let out = if self.block_state.regs.is_free(RCX) {
                        None
                    } else {
                        let new_reg = self.block_state.regs.take(I32);
                        dynasm!(self.asm
                            ; mov Rq(new_reg.rq().unwrap()), rcx
                        );
                        Some(new_reg)
                    };

                    match other {
                        ValueLocation::Reg(gpr) => {
                            dynasm!(self.asm
                                ; mov cl, Rb(gpr.rq().unwrap())
                            );
                        }
                        ValueLocation::Stack(offset) => {
                            let offset = self.adjusted_offset(offset);
                            dynasm!(self.asm
                                ; mov cl, [rsp + offset]
                            );
                        }
                        ValueLocation::Immediate(imm) => {
                            dynasm!(self.asm
                                ; mov cl, imm.as_int().unwrap() as i8
                            );
                        }
                    }

                    out
                }
            };

            self.free_value(count);
            self.block_state.regs.mark_used(RCX);
            count = ValueLocation::Reg(RCX);

            let reg = self.into_reg($ty, val);

            dynasm!(self.asm
                ; $instr $reg_ty(reg.rq().unwrap()), cl
            );

            self.free_value(count);

            if let Some(gpr) = temp_rcx {
                dynasm!(self.asm
                    ; mov rcx, Rq(gpr.rq().unwrap())
                );
                self.block_state.regs.release(gpr);
            }

            self.push(ValueLocation::Reg(reg));
        }
    }
}

macro_rules! cmp_i32 {
    ($name:ident, $instr:ident, $reverse_instr:ident, $const_fallback:expr) => {
        pub fn $name(&mut self) {
            let right = self.pop();
            let mut left = self.pop();

            let out = if let Some(i) = left.imm_i32() {
                match right {
                    ValueLocation::Stack(offset) => {
                        let result = self.block_state.regs.take(I32);
                        let offset = self.adjusted_offset(offset);

                        dynasm!(self.asm
                            ; xor Rd(result.rq().unwrap()), Rd(result.rq().unwrap())
                            ; cmp DWORD [rsp + offset], i
                            ; $reverse_instr Rb(result.rq().unwrap())
                        );
                        ValueLocation::Reg(result)
                    }
                    ValueLocation::Reg(rreg) => {
                        let result = self.block_state.regs.take(I32);
                        dynasm!(self.asm
                            ; xor Rd(result.rq().unwrap()), Rd(result.rq().unwrap())
                            ; cmp Rd(rreg.rq().unwrap()), i
                            ; $reverse_instr Rb(result.rq().unwrap())
                        );
                        ValueLocation::Reg(result)
                    }
                    ValueLocation::Immediate(right) => {
                        ValueLocation::Immediate(
                            (if $const_fallback(i, right.as_i32().unwrap()) {
                                1i32
                            } else {
                                0i32
                            }).into()
                        )
                    }
                }
            } else {
                let lreg = self.into_reg(I32, left);
                // TODO: Make `into_reg` take an `&mut`?
                left = ValueLocation::Reg(lreg);

                let result = self.block_state.regs.take(I32);

                match right {
                    ValueLocation::Stack(offset) => {
                        let offset = self.adjusted_offset(offset);
                        dynasm!(self.asm
                            ; xor Rd(result.rq().unwrap()), Rd(result.rq().unwrap())
                            ; cmp Rd(lreg.rq().unwrap()), [rsp + offset]
                            ; $instr Rb(result.rq().unwrap())
                        );
                    }
                    ValueLocation::Reg(rreg) => {
                        dynasm!(self.asm
                            ; xor Rd(result.rq().unwrap()), Rd(result.rq().unwrap())
                            ; cmp Rd(lreg.rq().unwrap()), Rd(rreg.rq().unwrap())
                            ; $instr Rb(result.rq().unwrap())
                        );
                    }
                    ValueLocation::Immediate(i) => {
                        dynasm!(self.asm
                            ; xor Rd(result.rq().unwrap()), Rd(result.rq().unwrap())
                            ; cmp Rd(lreg.rq().unwrap()), i.as_i32().unwrap()
                            ; $instr Rb(result.rq().unwrap())
                        );
                    }
                }

                ValueLocation::Reg(result)
            };

            self.free_value(left);
            self.free_value(right);

            self.push(out);
        }
    }
}

macro_rules! cmp_i64 {
    ($name:ident, $instr:ident, $reverse_instr:ident, $const_fallback:expr) => {
        pub fn $name(&mut self) {
            let right = self.pop();
            let mut left = self.pop();

            let out = if let Some(i) = left.imm_i64() {
                match right {
                    ValueLocation::Stack(offset) => {
                        let result = self.block_state.regs.take(I32);
                        let offset = self.adjusted_offset(offset);
                        if let Some(i) = i.try_into() {
                            dynasm!(self.asm
                                ; xor Rd(result.rq().unwrap()), Rd(result.rq().unwrap())
                                ; cmp QWORD [rsp + offset], i
                                ; $reverse_instr Rb(result.rq().unwrap())
                            );
                        } else {
                            unimplemented!("Unsupported `cmp` with large 64-bit immediate operand");
                        }
                        ValueLocation::Reg(result)
                    }
                    ValueLocation::Reg(rreg) => {
                        let result = self.block_state.regs.take(I32);
                        if let Some(i) = i.try_into() {
                            dynasm!(self.asm
                                ; xor Rd(result.rq().unwrap()), Rd(result.rq().unwrap())
                                ; cmp Rq(rreg.rq().unwrap()), i
                                ; $reverse_instr Rb(result.rq().unwrap())
                            );
                        } else {
                            unimplemented!("Unsupported `cmp` with large 64-bit immediate operand");
                        }
                        ValueLocation::Reg(result)
                    }
                    ValueLocation::Immediate(right) => {
                        ValueLocation::Immediate(
                            (if $const_fallback(i, right.as_i64().unwrap()) {
                                1i32
                            } else {
                                0i32
                            }).into()
                        )
                    }
                }
            } else {
                let lreg = self.into_reg(I64, left);
                left = ValueLocation::Reg(lreg);

                let result = self.block_state.regs.take(I32);

                match right {
                    ValueLocation::Stack(offset) => {
                        let offset = self.adjusted_offset(offset);
                        dynasm!(self.asm
                            ; xor Rd(result.rq().unwrap()), Rd(result.rq().unwrap())
                            ; cmp Rq(lreg.rq().unwrap()), [rsp + offset]
                            ; $instr Rb(result.rq().unwrap())
                        );
                    }
                    ValueLocation::Reg(rreg) => {
                        dynasm!(self.asm
                            ; xor Rd(result.rq().unwrap()), Rd(result.rq().unwrap())
                            ; cmp Rq(lreg.rq().unwrap()), Rq(rreg.rq().unwrap())
                            ; $instr Rb(result.rq().unwrap())
                        );
                    }
                    ValueLocation::Immediate(i) => {
                        let i = i.as_i64().unwrap();
                        if let Some(i) = i.try_into() {
                            dynasm!(self.asm
                                ; xor Rd(result.rq().unwrap()), Rd(result.rq().unwrap())
                                ; cmp Rq(lreg.rq().unwrap()), i
                                ; $instr Rb(result.rq().unwrap())
                            );
                        } else {
                            unimplemented!("Unsupported `cmp` with large 64-bit immediate operand");
                        }
                    }
                }

                ValueLocation::Reg(result)
            };

            self.free_value(left);
            self.free_value(right);
            self.push(out);
        }
    }
}

macro_rules! cmp_f32 {
    ($name:ident, $reverse_name:ident, $instr:ident, $const_fallback:expr) => {
        cmp_float!(
            comiss,
            f32,
            imm_f32,
            $name,
            $reverse_name,
            $instr,
            $const_fallback
        );
    };
}

macro_rules! cmp_f64 {
    ($name:ident, $reverse_name:ident, $instr:ident, $const_fallback:expr) => {
        cmp_float!(
            comisd,
            f64,
            imm_f64,
            $name,
            $reverse_name,
            $instr,
            $const_fallback
        );
    };
}

macro_rules! cmp_float {
    (@helper $cmp_instr:ident, $ty:ty, $imm_fn:ident, $self:expr, $left:expr, $right:expr, $instr:ident, $const_fallback:expr) => {{
        let (left, right, this) = ($left, $right, $self);
        if let (Some(left), Some(right)) = (left.$imm_fn(), right.$imm_fn()) {
            if $const_fallback(<$ty>::from_bits(left.bits()), <$ty>::from_bits(right.bits())) {
                ValueLocation::Immediate(1i32.into())
            } else {
                ValueLocation::Immediate(0i32.into())
            }
        } else {
            let lreg = this.into_reg(GPRType::Rx, *left);
            *left = ValueLocation::Reg(lreg);
            let result = this.block_state.regs.take(I32);

            match right {
                ValueLocation::Stack(offset) => {
                    let offset = this.adjusted_offset(*offset);

                    dynasm!(this.asm
                        ; xor Rq(result.rq().unwrap()), Rq(result.rq().unwrap())
                        ; $cmp_instr Rx(lreg.rx().unwrap()), [rsp + offset]
                        ; $instr Rb(result.rq().unwrap())
                    );
                }
                right => {
                    let rreg = this.into_reg(GPRType::Rx, *right);
                    *right = ValueLocation::Reg(rreg);

                    dynasm!(this.asm
                        ; xor Rq(result.rq().unwrap()), Rq(result.rq().unwrap())
                        ; $cmp_instr Rx(lreg.rx().unwrap()), Rx(rreg.rx().unwrap())
                        ; $instr Rb(result.rq().unwrap())
                    );
                }
            }

            ValueLocation::Reg(result)
        }
    }};
    ($cmp_instr:ident, $ty:ty, $imm_fn:ident, $name:ident, $reverse_name:ident, $instr:ident, $const_fallback:expr) => {
        pub fn $name(&mut self) {
            let mut right = self.pop();
            let mut left = self.pop();

            let out = cmp_float!(@helper
                $cmp_instr,
                $ty,
                $imm_fn,
                &mut *self,
                &mut left,
                &mut right,
                $instr,
                $const_fallback
            );

            self.free_value(left);
            self.free_value(right);

            self.push(out);
        }

        pub fn $reverse_name(&mut self) {
            let mut right = self.pop();
            let mut left = self.pop();

            let out = cmp_float!(@helper
                $cmp_instr,
                $ty,
                $imm_fn,
                &mut *self,
                &mut right,
                &mut left,
                $instr,
                $const_fallback
            );

            self.free_value(left);
            self.free_value(right);

            self.push(out);
        }
    };
}

macro_rules! binop_i32 {
    ($name:ident, $instr:ident, $const_fallback:expr) => {
        binop!(
            $name,
            $instr,
            $const_fallback,
            Rd,
            rq,
            I32,
            imm_i32,
            |this: &mut Context<_>, op1: GPR, i| dynasm!(this.asm
                ; $instr Rd(op1.rq().unwrap()), i
            )
        );
    };
}

macro_rules! commutative_binop_i32 {
    ($name:ident, $instr:ident, $const_fallback:expr) => {
        commutative_binop!(
            $name,
            $instr,
            $const_fallback,
            Rd,
            rq,
            I32,
            imm_i32,
            |this: &mut Context<_>, op1: GPR, i| dynasm!(this.asm
                ; $instr Rd(op1.rq().unwrap()), i
            )
        );
    };
}

macro_rules! binop_i64 {
    ($name:ident, $instr:ident, $const_fallback:expr) => {
        binop!(
            $name,
            $instr,
            $const_fallback,
            Rq,
            rq,
            I64,
            imm_i64,
            |this: &mut Context<_>, op1: GPR, i| dynasm!(this.asm
                ; $instr Rq(op1.rq().unwrap()), i
            )
        );
    };
}

macro_rules! commutative_binop_i64 {
    ($name:ident, $instr:ident, $const_fallback:expr) => {
        commutative_binop!(
            $name,
            $instr,
            $const_fallback,
            Rq,
            rq,
            I64,
            imm_i64,
            |this: &mut Context<_>, op1: GPR, i| dynasm!(this.asm
                ; $instr Rq(op1.rq().unwrap()), i
            )
        );
    };
}

macro_rules! binop_f32 {
    ($name:ident, $instr:ident, $const_fallback:expr) => {
        binop!(
            $name,
            $instr,
            |a: wasmparser::Ieee32, b: wasmparser::Ieee32| wasmparser::Ieee32(
                $const_fallback(f32::from_bits(a.bits()), f32::from_bits(b.bits())).to_bits()
            ),
            Rx,
            rx,
            F32,
            imm_f32,
            |_, _, _| unreachable!()
        );
    };
}

macro_rules! commutative_binop_f32 {
    ($name:ident, $instr:ident, $const_fallback:expr) => {
        commutative_binop!(
            $name,
            $instr,
            |a: wasmparser::Ieee32, b: wasmparser::Ieee32| wasmparser::Ieee32(
                $const_fallback(f32::from_bits(a.bits()), f32::from_bits(b.bits())).to_bits()
            ),
            Rx,
            rx,
            F32,
            imm_f32,
            |_, _, _| unreachable!()
        );
    };
}

macro_rules! binop_f64 {
    ($name:ident, $instr:ident, $const_fallback:expr) => {
        binop!(
            $name,
            $instr,
            |a: wasmparser::Ieee64, b: wasmparser::Ieee64| wasmparser::Ieee64(
                $const_fallback(f64::from_bits(a.bits()), f64::from_bits(b.bits())).to_bits()
            ),
            Rx,
            rx,
            F64,
            imm_f64,
            |_, _, _| unreachable!()
        );
    };
}

macro_rules! commutative_binop_f64 {
    ($name:ident, $instr:ident, $const_fallback:expr) => {
        commutative_binop!(
            $name,
            $instr,
            |a: wasmparser::Ieee64, b: wasmparser::Ieee64| wasmparser::Ieee64(
                $const_fallback(f64::from_bits(a.bits()), f64::from_bits(b.bits())).to_bits()
            ),
            Rx,
            rx,
            F64,
            imm_f64,
            |_, _, _| unreachable!()
        );
    };
}
macro_rules! commutative_binop {
    ($name:ident, $instr:ident, $const_fallback:expr, $reg_ty:ident, $reg_fn:ident, $ty:expr, $imm_fn:ident, $direct_imm:expr) => {
        binop!(
            $name,
            $instr,
            $const_fallback,
            $reg_ty,
            $reg_fn,
            $ty,
            $imm_fn,
            $direct_imm,
            |op1: ValueLocation, op0: ValueLocation| match op1 {
                ValueLocation::Reg(_) => (op1, op0),
                _ => {
                    if op0.immediate().is_some() {
                        (op1, op0)
                    } else {
                        (op0, op1)
                    }
                }
            }
        );
    };
}

macro_rules! binop {
    ($name:ident, $instr:ident, $const_fallback:expr, $reg_ty:ident, $reg_fn:ident, $ty:expr, $imm_fn:ident, $direct_imm:expr) => {
        binop!($name, $instr, $const_fallback, $reg_ty, $reg_fn, $ty, $imm_fn, $direct_imm, |a, b| (a, b));
    };
    ($name:ident, $instr:ident, $const_fallback:expr, $reg_ty:ident, $reg_fn:ident, $ty:expr, $imm_fn:ident, $direct_imm:expr, $map_op:expr) => {
        pub fn $name(&mut self) {
            let right = self.pop();
            let left = self.pop();

            if let Some(i1) = left.$imm_fn() {
                if let Some(i0) = right.$imm_fn() {
                    self.block_state.stack.push(ValueLocation::Immediate($const_fallback(i1, i0).into()));
                    return;
                }
            }

            let (left, mut right) = $map_op(left, right);
            let left = self.into_temp_reg($ty, left);

            match right {
                ValueLocation::Reg(_) => {
                    // This handles the case where we (for example) have a float in an `Rq` reg
                    let right_reg = self.into_reg($ty, right);
                    right = ValueLocation::Reg(right_reg);
                    dynasm!(self.asm
                        ; $instr $reg_ty(left.$reg_fn().unwrap()), $reg_ty(right_reg.$reg_fn().unwrap())
                    );
                }
                ValueLocation::Stack(offset) => {
                    let offset = self.adjusted_offset(offset);
                    dynasm!(self.asm
                        ; $instr $reg_ty(left.$reg_fn().unwrap()), [rsp + offset]
                    );
                }
                ValueLocation::Immediate(i) => {
                    if let Some(i) = i.as_int().and_then(|i| i.try_into()) {
                        $direct_imm(self, left, i);
                    } else {
                        let scratch = self.block_state.regs.take($ty);
                        self.immediate_to_reg(scratch, i);

                        dynasm!(self.asm
                            ; $instr $reg_ty(left.$reg_fn().unwrap()), $reg_ty(scratch.$reg_fn().unwrap())
                        );

                        self.block_state.regs.release(scratch);
                    }
                }
            }

            self.free_value(right);
            self.push(ValueLocation::Reg(left));
        }
    }
}

macro_rules! load {
    (@inner $name:ident, $rtype:expr, $reg_ty:ident, $emit_fn:expr) => {
        pub fn $name(&mut self, offset: u32) {
            fn load_to_reg<_M: ModuleContext>(
                ctx: &mut Context<_M>,
                dst: GPR,
                (offset, runtime_offset): (i32, Result<i32, GPR>)
            ) {
                let vmctx_mem_ptr_offset = ctx.module_context

                    .vmctx_vmmemory_definition_base(0) as i32;
                let mem_ptr_reg = ctx.block_state.regs.take(I64);
                dynasm!(ctx.asm
                    ; mov Rq(mem_ptr_reg.rq().unwrap()), [Rq(VMCTX) + vmctx_mem_ptr_offset]
                );
                $emit_fn(ctx, dst, mem_ptr_reg, runtime_offset, offset);
                ctx.block_state.regs.release(mem_ptr_reg);
            }

            let base = self.pop();

            let temp = self.block_state.regs.take($rtype);

            match base {
                ValueLocation::Immediate(i) => {
                    load_to_reg(self, temp, (offset as _, Ok(i.as_i32().unwrap())));
                }
                base => {
                    let gpr = self.into_reg(I32, base);
                    load_to_reg(self, temp, (offset as _, Err(gpr)));
                    self.block_state.regs.release(gpr);
                }
            }

            self.push(ValueLocation::Reg(temp));
        }
    };
    ($name:ident, $rtype:expr, $reg_ty:ident, NONE, $rq_instr:ident, $ty:ident) => {
        load!(@inner
            $name,
            $rtype,
            $reg_ty,
            |ctx: &mut Context<_>, dst: GPR, mem_ptr_reg: GPR, runtime_offset: Result<i32, GPR>, offset: i32| {
                match runtime_offset {
                    Ok(imm) => {
                        dynasm!(ctx.asm
                            ; $rq_instr $reg_ty(dst.rq().unwrap()), $ty [Rq(mem_ptr_reg.rq().unwrap()) + offset + imm]
                        );
                    }
                    Err(offset_reg) => {
                        dynasm!(ctx.asm
                            ; $rq_instr $reg_ty(dst.rq().unwrap()), $ty [Rq(mem_ptr_reg.rq().unwrap()) + Rq(offset_reg.rq().unwrap()) + offset]
                        );
                    }
                }
            }
        );
    };
    ($name:ident, $rtype:expr, $reg_ty:ident, $xmm_instr:ident, $rq_instr:ident, $ty:ident) => {
        load!(@inner
            $name,
            $rtype,
            $reg_ty,
            |ctx: &mut Context<_>, dst: GPR, mem_ptr_reg: GPR, runtime_offset: Result<i32, GPR>, offset: i32| {
                match (dst, runtime_offset) {
                    (GPR::Rq(r), Ok(imm)) => {
                        dynasm!(ctx.asm
                            ; $rq_instr $reg_ty(r), $ty [Rq(mem_ptr_reg.rq().unwrap()) + offset + imm]
                        );
                    }
                    (GPR::Rx(r), Ok(imm)) => {
                        dynasm!(ctx.asm
                            ; $xmm_instr Rx(r), $ty [Rq(mem_ptr_reg.rq().unwrap()) + offset + imm]
                        );
                    }
                    (GPR::Rq(r), Err(offset_reg)) => {
                        dynasm!(ctx.asm
                            ; $rq_instr $reg_ty(r), $ty [Rq(mem_ptr_reg.rq().unwrap()) + Rq(offset_reg.rq().unwrap()) + offset]
                        );
                    }
                    (GPR::Rx(r), Err(offset_reg)) => {
                        dynasm!(ctx.asm
                            ; $xmm_instr Rx(r), $ty [Rq(mem_ptr_reg.rq().unwrap()) + Rq(offset_reg.rq().unwrap()) + offset]
                        );
                    }
                }
            }
        );
    };
}

macro_rules! store {
    (@inner $name:ident, $int_reg_ty:ident, $match_offset:expr, $size:ident) => {
        pub fn $name(&mut self, offset: u32) {
            fn store_from_reg<_M: ModuleContext>(
                ctx: &mut Context<_M>,
                src: GPR,
                (offset, runtime_offset): (i32, Result<i32, GPR>)
            ) {
                let vmctx_mem_ptr_offset = ctx.module_context

                    .vmctx_vmmemory_definition_base(0) as i32;
                let mem_ptr_reg = ctx.block_state.regs.take(GPRType::Rq);
                dynasm!(ctx.asm
                    ; mov Rq(mem_ptr_reg.rq().unwrap()), [Rq(VMCTX) + vmctx_mem_ptr_offset]
                );
                let src = $match_offset(ctx, mem_ptr_reg, runtime_offset, offset, src);
                ctx.block_state.regs.release(mem_ptr_reg);

                ctx.block_state.regs.release(src);
            }

            assert!(offset <= i32::max_value() as u32);

            let src = self.pop();
            let base = self.pop();

            let src_reg = self.into_reg(None, src);

            match base {
                ValueLocation::Immediate(i) => {
                    store_from_reg(self, src_reg, (offset as i32, Ok(i.as_i32().unwrap())));
                }
                base => {
                    let gpr = self.into_reg(I32, base);
                    store_from_reg(self, src_reg, (offset as i32, Err(gpr)));
                    self.block_state.regs.release(gpr);
                }
            }
        }
    };
    ($name:ident, $int_reg_ty:ident, NONE, $size:ident) => {
        store!(@inner
            $name,
            $int_reg_ty,
            |ctx: &mut Context<_>, mem_ptr_reg: GPR, runtime_offset: Result<i32, GPR>, offset: i32, src| {
                let src_reg = ctx.into_temp_reg(GPRType::Rq, ValueLocation::Reg(src));

                match runtime_offset {
                    Ok(imm) => {
                        dynasm!(ctx.asm
                            ; mov [Rq(mem_ptr_reg.rq().unwrap()) + offset + imm], $int_reg_ty(src_reg.rq().unwrap())
                        );
                    }
                    Err(offset_reg) => {
                        dynasm!(ctx.asm
                            ; mov [Rq(mem_ptr_reg.rq().unwrap()) + Rq(offset_reg.rq().unwrap()) + offset], $int_reg_ty(src_reg.rq().unwrap())
                        );
                    }
                }

                src_reg
            },
            $size
        );
    };
    ($name:ident, $int_reg_ty:ident, $xmm_instr:ident, $size:ident) => {
        store!(@inner
            $name,
            $int_reg_ty,
            |ctx: &mut Context<_>, mem_ptr_reg: GPR, runtime_offset: Result<i32, GPR>, offset: i32, src| {
                match (runtime_offset, src) {
                    (Ok(imm), GPR::Rq(r)) => {
                        dynasm!(ctx.asm
                            ; mov [Rq(mem_ptr_reg.rq().unwrap()) + offset + imm], $int_reg_ty(r)
                        );
                    }
                    (Ok(imm), GPR::Rx(r)) => {
                        dynasm!(ctx.asm
                            ; $xmm_instr [Rq(mem_ptr_reg.rq().unwrap()) + offset + imm], Rx(r)
                        );
                    }
                    (Err(offset_reg), GPR::Rq(r)) => {
                        dynasm!(ctx.asm
                            ; mov [Rq(mem_ptr_reg.rq().unwrap()) + Rq(offset_reg.rq().unwrap()) + offset], $int_reg_ty(r)
                        );
                    }
                    (Err(offset_reg), GPR::Rx(r)) => {
                        dynasm!(ctx.asm
                            ; $xmm_instr [Rq(mem_ptr_reg.rq().unwrap()) + Rq(offset_reg.rq().unwrap()) + offset], Rx(r)
                        );
                    }
                }

                src
            },
            $size
        );
    };
}

trait TryInto<O> {
    fn try_into(self) -> Option<O>;
}

impl TryInto<i64> for u64 {
    fn try_into(self) -> Option<i64> {
        let max = i64::max_value() as u64;

        if self <= max {
            Some(self as i64)
        } else {
            None
        }
    }
}

impl TryInto<i32> for i64 {
    fn try_into(self) -> Option<i32> {
        let min = i32::min_value() as i64;
        let max = i32::max_value() as i64;

        if self >= min && self <= max {
            Some(self as i32)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct VirtualCallingConvention {
    pub stack: Stack,
    pub depth: StackDepth,
}

impl<'module, M: ModuleContext> Context<'module, M> {
    pub fn debug(&mut self, d: std::fmt::Arguments) {
        asm_println!(self.asm, "{}", d);
    }

    pub fn virtual_calling_convention(&self) -> VirtualCallingConvention {
        VirtualCallingConvention {
            stack: self.block_state.stack.clone(),
            depth: self.block_state.depth,
        }
    }

    /// Create a new undefined label.
    pub fn create_label(&mut self) -> Label {
        Label(self.asm.new_dynamic_label())
    }

    pub fn define_host_fn(&mut self, host_fn: *const u8) {
        dynasm!(self.asm
            ; mov rax, QWORD host_fn as i64
            ; call rax
            ; ret
        );
    }

    fn adjusted_offset(&self, offset: i32) -> i32 {
        (self.block_state.depth.0 as i32 + offset) * WORD_SIZE as i32
    }

    fn zero_reg(&mut self, reg: GPR) {
        match reg {
            GPR::Rq(r) => {
                dynasm!(self.asm
                    ; xor Rq(r), Rq(r)
                );
            }
            GPR::Rx(r) => {
                dynasm!(self.asm
                    ; pxor Rx(r), Rx(r)
                );
            }
        }
    }

    cmp_i32!(i32_eq, sete, sete, |a, b| a == b);
    cmp_i32!(i32_neq, setne, setne, |a, b| a != b);
    // `dynasm-rs` inexplicably doesn't support setb but `setnae` (and `setc`) are synonymous
    cmp_i32!(i32_lt_u, setnae, seta, |a, b| (a as u32) < (b as u32));
    cmp_i32!(i32_le_u, setbe, setae, |a, b| (a as u32) <= (b as u32));
    cmp_i32!(i32_gt_u, seta, setnae, |a, b| (a as u32) > (b as u32));
    cmp_i32!(i32_ge_u, setae, setna, |a, b| (a as u32) >= (b as u32));
    cmp_i32!(i32_lt_s, setl, setnle, |a, b| a < b);
    cmp_i32!(i32_le_s, setle, setnl, |a, b| a <= b);
    cmp_i32!(i32_gt_s, setg, setnge, |a, b| a > b);
    cmp_i32!(i32_ge_s, setge, setng, |a, b| a >= b);

    cmp_i64!(i64_eq, sete, sete, |a, b| a == b);
    cmp_i64!(i64_neq, setne, setne, |a, b| a != b);
    // `dynasm-rs` inexplicably doesn't support setb but `setnae` (and `setc`) are synonymous
    cmp_i64!(i64_lt_u, setnae, seta, |a, b| (a as u64) < (b as u64));
    cmp_i64!(i64_le_u, setbe, setae, |a, b| (a as u64) <= (b as u64));
    cmp_i64!(i64_gt_u, seta, setnae, |a, b| (a as u64) > (b as u64));
    cmp_i64!(i64_ge_u, setae, setna, |a, b| (a as u64) >= (b as u64));
    cmp_i64!(i64_lt_s, setl, setnle, |a, b| a < b);
    cmp_i64!(i64_le_s, setle, setnl, |a, b| a <= b);
    cmp_i64!(i64_gt_s, setg, setnge, |a, b| a > b);
    cmp_i64!(i64_ge_s, setge, setng, |a, b| a >= b);

    cmp_f32!(f32_gt, f32_lt, seta, |a, b| a > b);
    cmp_f32!(f32_ge, f32_le, setnc, |a, b| a >= b);

    cmp_f64!(f64_gt, f64_lt, seta, |a, b| a > b);
    cmp_f64!(f64_ge, f64_le, setnc, |a, b| a >= b);

    // TODO: Should we do this logic in `eq` and just have this delegate to `eq`?
    //       That would mean that `eqz` and `eq` with a const 0 argument don't
    //       result in different code. It would also allow us to generate better
    //       code for `neq` and `gt_u` with const 0 operand
    pub fn i32_eqz(&mut self) {
        let val = self.pop();

        if let ValueLocation::Immediate(Value::I32(i)) = val {
            self.push(ValueLocation::Immediate(
                (if i == 0 { 1i32 } else { 0 }).into(),
            ));
            return;
        }

        let reg = self.into_reg(I32, val);
        let out = self.block_state.regs.take(I32);

        dynasm!(self.asm
            ; xor Rd(out.rq().unwrap()), Rd(out.rq().unwrap())
            ; test Rd(reg.rq().unwrap()), Rd(reg.rq().unwrap())
            ; setz Rb(out.rq().unwrap())
        );

        self.block_state.regs.release(reg);

        self.push(ValueLocation::Reg(out));
    }

    pub fn i64_eqz(&mut self) {
        let val = self.pop();

        if let ValueLocation::Immediate(Value::I64(i)) = val {
            self.push(ValueLocation::Immediate(
                (if i == 0 { 1i32 } else { 0 }).into(),
            ));
            return;
        }

        let reg = self.into_reg(I64, val);
        let out = self.block_state.regs.take(I64);

        dynasm!(self.asm
            ; xor Rd(out.rq().unwrap()), Rd(out.rq().unwrap())
            ; test Rq(reg.rq().unwrap()), Rq(reg.rq().unwrap())
            ; setz Rb(out.rq().unwrap())
        );

        self.block_state.regs.release(reg);

        self.push(ValueLocation::Reg(out));
    }

    /// Pops i32 predicate and branches to the specified label
    /// if the predicate is equal to zero.
    pub fn br_if_false(
        &mut self,
        target: impl Into<BrTarget<Label>>,
        pass_args: impl FnOnce(&mut Self),
    ) {
        let val = self.pop();
        let label = target
            .into()
            .label()
            .map(|c| *c)
            .unwrap_or_else(|| self.ret_label());

        pass_args(self);

        let predicate = self.into_reg(I32, val);

        dynasm!(self.asm
            ; test Rd(predicate.rq().unwrap()), Rd(predicate.rq().unwrap())
            ; jz =>label.0
        );

        self.block_state.regs.release(predicate);
    }

    /// Pops i32 predicate and branches to the specified label
    /// if the predicate is not equal to zero.
    pub fn br_if_true(
        &mut self,
        target: impl Into<BrTarget<Label>>,
        pass_args: impl FnOnce(&mut Self),
    ) {
        let val = self.pop();
        let label = target
            .into()
            .label()
            .map(|c| *c)
            .unwrap_or_else(|| self.ret_label());

        pass_args(self);

        let predicate = self.into_reg(I32, val);

        dynasm!(self.asm
            ; test Rd(predicate.rq().unwrap()), Rd(predicate.rq().unwrap())
            ; jnz =>label.0
        );

        self.block_state.regs.release(predicate);
    }

    /// Branch unconditionally to the specified label.
    pub fn br(&mut self, label: impl Into<BrTarget<Label>>) {
        match label.into() {
            BrTarget::Return => self.ret(),
            BrTarget::Label(label) => dynasm!(self.asm
                ; jmp =>label.0
            ),
        }
    }

    /// If `default` is `None` then the default is just continuing execution
    pub fn br_table<I>(
        &mut self,
        targets: I,
        default: Option<BrTarget<Label>>,
        pass_args: impl FnOnce(&mut Self),
    ) where
        I: IntoIterator<Item = BrTarget<Label>>,
        I::IntoIter: ExactSizeIterator,
    {
        let mut targets = targets.into_iter();
        let count = targets.len();

        let mut selector = self.pop();

        pass_args(self);

        if let Some(imm) = selector.imm_i32() {
            if let Some(target) = targets.nth(imm as _).or(default) {
                match target {
                    BrTarget::Label(label) => self.br(label),
                    BrTarget::Return => {
                        dynasm!(self.asm
                            ; ret
                        );
                    }
                }
            }
        } else {
            if count > 0 {
                let selector_reg = self.into_reg(GPRType::Rq, selector);
                selector = ValueLocation::Reg(selector_reg);

                let tmp = self.block_state.regs.take(I64);

                self.immediate_to_reg(tmp, (count as u32).into());
                dynasm!(self.asm
                    ; cmp Rq(selector_reg.rq().unwrap()), Rq(tmp.rq().unwrap())
                    ; cmova Rq(selector_reg.rq().unwrap()), Rq(tmp.rq().unwrap())
                    ; lea Rq(tmp.rq().unwrap()), [>start_label]
                    ; lea Rq(selector_reg.rq().unwrap()), [
                        Rq(selector_reg.rq().unwrap()) * 5
                    ]
                    ; add Rq(selector_reg.rq().unwrap()), Rq(tmp.rq().unwrap())
                    ; jmp Rq(selector_reg.rq().unwrap())
                    ; start_label:
                );

                self.block_state.regs.release(tmp);

                for (i, target) in targets.enumerate() {
                    let label = self.target_to_label(target);
                    dynasm!(self.asm
                        ; jmp =>label.0
                    );
                }
            }

            if let Some(def) = default {
                match def {
                    BrTarget::Label(label) => dynasm!(self.asm
                        ; jmp =>label.0
                    ),
                    BrTarget::Return => dynasm!(self.asm
                        ; ret
                    ),
                }
            }
        }

        self.free_value(selector);
    }

    fn set_stack_depth_preserve_flags(&mut self, depth: StackDepth) {
        if self.block_state.depth.0 < depth.0 {
            // TODO: We need to preserve ZF on `br_if` so we use `push`/`pop` but that isn't
            //       necessary on (for example) `br`.
            for _ in 0..depth.0 - self.block_state.depth.0 {
                dynasm!(self.asm
                    ; push rax
                );
            }
        } else if self.block_state.depth.0 > depth.0 {
            let trash = self.block_state.regs.take(I64);
            // TODO: We need to preserve ZF on `br_if` so we use `push`/`pop` but that isn't
            //       necessary on (for example) `br`.
            for _ in 0..self.block_state.depth.0 - depth.0 {
                dynasm!(self.asm
                    ; pop Rq(trash.rq().unwrap())
                );
            }
        }

        self.block_state.depth = depth;
    }

    fn set_stack_depth(&mut self, depth: StackDepth) {
        if self.block_state.depth.0 != depth.0 {
            let diff = depth.0 as i32 - self.block_state.depth.0 as i32;
            if diff.abs() == 1 {
                self.set_stack_depth_preserve_flags(depth);
            } else {
                dynasm!(self.asm
                    ; add rsp, (self.block_state.depth.0 as i32 - depth.0 as i32) * WORD_SIZE as i32
                );

                self.block_state.depth = depth;
            }
        }
    }

    pub fn pass_block_args(&mut self, cc: &CallingConvention) {
        let args = &cc.arguments;
        for (remaining, &dst) in args
            .iter()
            .enumerate()
            .rev()
            .take(self.block_state.stack.len())
        {
            if let CCLoc::Reg(r) = dst {
                if !self.block_state.regs.is_free(r)
                    && *self.block_state.stack.last().unwrap() != ValueLocation::Reg(r)
                {
                    // TODO: This would be made simpler and more efficient with a proper SSE
                    //       representation.
                    self.save_regs(&[r], ..=remaining);
                }

                self.block_state.regs.mark_used(r);
            }
            self.pop_into(dst.into());
        }

        self.set_stack_depth(cc.stack_depth);
    }

    /// Puts all stack values into "real" locations so that they can i.e. be set to different
    /// values on different iterations of a loop
    pub fn serialize_args(&mut self, count: u32) -> CallingConvention {
        let mut out = Vec::with_capacity(count as _);

        // TODO: We can make this more efficient now that `pop` isn't so complicated
        for _ in 0..count {
            let val = self.pop();
            // TODO: We can use stack slots for values already on the stack but we
            //       don't refcount stack slots right now
            let loc = CCLoc::Reg(self.into_temp_reg(None, val));

            out.push(loc);
        }

        out.reverse();

        CallingConvention {
            stack_depth: self.block_state.depth,
            arguments: out,
        }
    }

    pub fn get_global(&mut self, global_idx: u32) {
        let offset = self.module_context.vmctx_vmglobal_definition(
            self.module_context
                .defined_global_index(global_idx)
                .expect("TODO: Support imported globals"),
        );

        let out = self.block_state.regs.take(GPRType::Rq);

        // We always use `Rq` (even for floats) since the globals are not necessarily aligned to 128 bits
        dynasm!(self.asm
            ; mov Rq(out.rq().unwrap()), [Rq(VMCTX) + offset as i32]
        );

        self.push(ValueLocation::Reg(out));
    }

    pub fn set_global(&mut self, global_idx: u32) {
        let val = self.pop();
        let offset = self.module_context.vmctx_vmglobal_definition(
            self.module_context
                .defined_global_index(global_idx)
                .expect("TODO: Support imported globals"),
        );

        let val = self.into_reg(GPRType::Rq, val);

        // We always use `Rq` (even for floats) since the globals are not necessarily aligned to 128 bits
        dynasm!(self.asm
            ; mov [Rq(VMCTX) + offset as i32], Rq(val.rq().unwrap())
        );

        self.block_state.regs.release(val);
    }

    fn immediate_to_reg(&mut self, reg: GPR, val: Value) {
        if val.as_bytes() == 0 {
            self.zero_reg(reg);
        } else {
            match reg {
                GPR::Rq(r) => {
                    let val = val.as_bytes();
                    if (val as u64) <= u32::max_value() as u64 {
                        dynasm!(self.asm
                            ; mov Rd(r), val as i32
                        );
                    } else {
                        dynasm!(self.asm
                            ; mov Rq(r), QWORD val
                        );
                    }
                }
                GPR::Rx(r) => {
                    let temp = self.block_state.regs.take(I64);
                    self.immediate_to_reg(temp, val);
                    dynasm!(self.asm
                        ; movq Rx(r), Rq(temp.rq().unwrap())
                    );
                    self.block_state.regs.release(temp);
                }
            }
        }
    }

    // The `&` and `&mut` aren't necessary (`ValueLocation` is copy) but it ensures that we don't get
    // the arguments the wrong way around. In the future we want to have a `ReadLocation` and `WriteLocation`
    // so we statically can't write to a literal so this will become a non-issue.
    fn copy_value(&mut self, src: &ValueLocation, dst: &mut ValueLocation) {
        match (*src, *dst) {
            (ValueLocation::Stack(in_offset), ValueLocation::Stack(out_offset)) => {
                let in_offset = self.adjusted_offset(in_offset);
                let out_offset = self.adjusted_offset(out_offset);
                if in_offset != out_offset {
                    let gpr = self.block_state.regs.take(I64);
                    dynasm!(self.asm
                        ; mov Rq(gpr.rq().unwrap()), [rsp + in_offset]
                        ; mov [rsp + out_offset], Rq(gpr.rq().unwrap())
                    );
                    self.block_state.regs.release(gpr);
                }
            }
            // TODO: XMM registers
            (ValueLocation::Reg(in_reg), ValueLocation::Stack(out_offset)) => {
                let out_offset = self.adjusted_offset(out_offset);
                match in_reg {
                    GPR::Rq(in_reg) => {
                        // We can always use `Rq` here for now because stack slots are in multiples of
                        // 8 bytes
                        dynasm!(self.asm
                            ; mov [rsp + out_offset], Rq(in_reg)
                        );
                    }
                    GPR::Rx(in_reg) => {
                        // We can always use `movq` here for now because stack slots are in multiples of
                        // 8 bytes
                        dynasm!(self.asm
                            ; movq [rsp + out_offset], Rx(in_reg)
                        );
                    }
                }
            }
            (ValueLocation::Immediate(i), ValueLocation::Stack(out_offset)) => {
                // TODO: Floats
                let i = i.as_bytes();
                let out_offset = self.adjusted_offset(out_offset);
                if (i as u64) <= u32::max_value() as u64 {
                    dynasm!(self.asm
                        ; mov DWORD [rsp + out_offset], i as i32
                    );
                } else {
                    let scratch = self.block_state.regs.take(I64);

                    dynasm!(self.asm
                        ; mov Rq(scratch.rq().unwrap()), QWORD i
                        ; mov [rsp + out_offset], Rq(scratch.rq().unwrap())
                    );

                    self.block_state.regs.release(scratch);
                }
            }
            (ValueLocation::Stack(in_offset), ValueLocation::Reg(out_reg)) => {
                let in_offset = self.adjusted_offset(in_offset);
                match out_reg {
                    GPR::Rq(out_reg) => {
                        // We can always use `Rq` here for now because stack slots are in multiples of
                        // 8 bytes
                        dynasm!(self.asm
                            ; mov Rq(out_reg), [rsp + in_offset]
                        );
                    }
                    GPR::Rx(out_reg) => {
                        // We can always use `movq` here for now because stack slots are in multiples of
                        // 8 bytes
                        dynasm!(self.asm
                            ; movq Rx(out_reg), [rsp + in_offset]
                        );
                    }
                }
            }
            (ValueLocation::Reg(in_reg), ValueLocation::Reg(out_reg)) => {
                if in_reg != out_reg {
                    match (in_reg, out_reg) {
                        (GPR::Rq(in_reg), GPR::Rq(out_reg)) => {
                            dynasm!(self.asm
                                ; mov Rq(out_reg), Rq(in_reg)
                            );
                        }
                        (GPR::Rx(in_reg), GPR::Rq(out_reg)) => {
                            dynasm!(self.asm
                                ; movq Rq(out_reg), Rx(in_reg)
                            );
                        }
                        (GPR::Rq(in_reg), GPR::Rx(out_reg)) => {
                            dynasm!(self.asm
                                ; movq Rx(out_reg), Rq(in_reg)
                            );
                        }
                        (GPR::Rx(in_reg), GPR::Rx(out_reg)) => {
                            dynasm!(self.asm
                                ; movq Rx(out_reg), Rx(in_reg)
                            );
                        }
                    }
                }
            }
            (ValueLocation::Immediate(i), ValueLocation::Reg(out_reg)) => {
                // TODO: Floats
                self.immediate_to_reg(out_reg, i);
            }
            // TODO: Have separate `ReadLocation` and `WriteLocation`?
            (_, ValueLocation::Immediate(_)) => panic!("Tried to copy to an immediate value!"),
        }
    }

    /// Define the given label at the current position.
    ///
    /// Multiple labels can be defined at the same position. However, a label
    /// can be defined only once.
    pub fn define_label(&mut self, label: Label) {
        self.asm.dynamic_label(label.0);
    }

    pub fn set_state(&mut self, state: VirtualCallingConvention) {
        self.block_state.regs = Registers::new();
        for elem in &state.stack {
            if let ValueLocation::Reg(r) = elem {
                self.block_state.regs.mark_used(*r);
            }
        }
        self.block_state.stack = state.stack;
        self.block_state.depth = state.depth;
    }

    pub fn apply_cc(&mut self, cc: &CallingConvention) {
        let stack = cc.arguments.iter();

        self.block_state.stack = Vec::with_capacity(stack.size_hint().0);
        self.block_state.regs = Registers::new();

        for &elem in stack {
            if let CCLoc::Reg(r) = elem {
                self.block_state.regs.mark_used(r);
            }

            self.block_state.stack.push(elem.into());
        }

        self.block_state.depth = cc.stack_depth;
    }

    load!(i32_load, GPRType::Rq, Rd, movd, mov, DWORD);
    load!(i64_load, GPRType::Rq, Rq, movq, mov, QWORD);
    load!(f32_load, GPRType::Rx, Rd, movd, mov, DWORD);
    load!(f64_load, GPRType::Rx, Rq, movq, mov, QWORD);

    load!(i32_load8_u, GPRType::Rq, Rd, NONE, movzx, BYTE);
    load!(i32_load8_s, GPRType::Rq, Rd, NONE, movsx, BYTE);
    load!(i32_load16_u, GPRType::Rq, Rd, NONE, movzx, WORD);
    load!(i32_load16_s, GPRType::Rq, Rd, NONE, movsx, WORD);

    load!(i64_load8_u, GPRType::Rq, Rq, NONE, movzx, BYTE);
    load!(i64_load8_s, GPRType::Rq, Rq, NONE, movsx, BYTE);
    load!(i64_load16_u, GPRType::Rq, Rq, NONE, movzx, WORD);
    load!(i64_load16_s, GPRType::Rq, Rq, NONE, movsx, WORD);
    load!(i64_load32_u, GPRType::Rq, Rd, movd, mov, DWORD);
    load!(i64_load32_s, GPRType::Rq, Rq, NONE, movsxd, DWORD);

    store!(store8, Rb, NONE, DWORD);
    store!(store16, Rw, NONE, QWORD);
    store!(store32, Rd, movd, DWORD);
    store!(store64, Rq, movq, QWORD);

    fn push_physical(&mut self, value: ValueLocation) -> ValueLocation {
        self.block_state.depth.reserve(1);
        match value {
            ValueLocation::Reg(gpr) => {
                // TODO: Proper stack allocation scheme
                dynasm!(self.asm
                    ; push Rq(gpr.rq().unwrap())
                );
                self.block_state.regs.release(gpr);
            }
            ValueLocation::Stack(o) => {
                let offset = self.adjusted_offset(o);
                dynasm!(self.asm
                    ; push QWORD [rsp + offset]
                );
            }
            ValueLocation::Immediate(imm) => {
                let gpr = self.block_state.regs.take(I64);
                dynasm!(self.asm
                    ; mov Rq(gpr.rq().unwrap()), QWORD imm.as_bytes()
                    ; push Rq(gpr.rq().unwrap())
                );
                self.block_state.regs.release(gpr);
            }
        }
        ValueLocation::Stack(-(self.block_state.depth.0 as i32))
    }

    fn push(&mut self, value: ValueLocation) {
        self.block_state.stack.push(value);
    }

    fn pop(&mut self) -> ValueLocation {
        self.block_state.stack.pop().expect("Stack is empty")
    }

    pub fn drop(&mut self, range: RangeInclusive<u32>) {
        let mut repush = Vec::with_capacity(*range.start() as _);

        for _ in 0..*range.start() {
            repush.push(self.pop());
        }

        for _ in range {
            let val = self.pop();
            self.free_value(val);
        }

        for v in repush.into_iter().rev() {
            self.push(v);
        }
    }

    fn pop_into(&mut self, dst: ValueLocation) {
        let val = self.pop();
        self.copy_value(&val, &mut { dst });
        self.free_value(val);
    }

    fn free_value(&mut self, val: ValueLocation) {
        match val {
            ValueLocation::Reg(r) => {
                self.block_state.regs.release(r);
            }
            // TODO: Refcounted stack slots
            _ => {}
        }
    }

    /// Puts this value into a register so that it can be efficiently read
    // TODO: We should allow choosing which reg type we want to allocate here (Rx/Rq)
    fn into_reg(&mut self, ty: impl Into<Option<GPRType>>, val: ValueLocation) -> GPR {
        let ty = ty.into();
        match val {
            ValueLocation::Reg(r) if ty.map(|t| t == r.type_()).unwrap_or(true) => r,
            val => {
                let scratch = self.block_state.regs.take(ty.unwrap_or(GPRType::Rq));

                self.copy_value(&val, &mut ValueLocation::Reg(scratch));
                self.free_value(val);

                scratch
            }
        }
    }

    /// Puts this value into a temporary register so that operations
    /// on that register don't write to a local.
    fn into_temp_reg(&mut self, ty: impl Into<Option<GPRType>>, val: ValueLocation) -> GPR {
        // If we have `None` as the type then it always matches (`.unwrap_or(true)`)
        match val {
            ValueLocation::Reg(r) => {
                let ty = ty.into();
                let type_matches = ty.map(|t| t == r.type_()).unwrap_or(true);

                if self.block_state.regs.num_usages(r) <= 1 && type_matches {
                    r
                } else {
                    let scratch = self.block_state.regs.take(ty.unwrap_or(GPRType::Rq));

                    self.copy_value(&val, &mut ValueLocation::Reg(scratch));
                    self.free_value(val);

                    scratch
                }
            }
            val => self.into_reg(ty, val),
        }
    }

    pub fn f32_neg(&mut self) {
        let val = self.pop();

        let out = if let Some(i) = val.imm_f32() {
            ValueLocation::Immediate(
                wasmparser::Ieee32((-f32::from_bits(i.bits())).to_bits()).into(),
            )
        } else {
            let reg = self.into_temp_reg(GPRType::Rx, val);
            let const_label = self.neg_const_f32_label();

            dynasm!(self.asm
                ; xorps Rx(reg.rx().unwrap()), [=>const_label.0]
            );

            ValueLocation::Reg(reg)
        };

        self.push(out);
    }

    pub fn f64_neg(&mut self) {
        let val = self.pop();

        let out = if let Some(i) = val.imm_f64() {
            ValueLocation::Immediate(
                wasmparser::Ieee64((-f64::from_bits(i.bits())).to_bits()).into(),
            )
        } else {
            let reg = self.into_temp_reg(GPRType::Rx, val);
            let const_label = self.neg_const_f64_label();

            dynasm!(self.asm
                ; xorpd Rx(reg.rx().unwrap()), [=>const_label.0]
            );

            ValueLocation::Reg(reg)
        };

        self.push(out);
    }

    pub fn f32_abs(&mut self) {
        let val = self.pop();

        let out = if let Some(i) = val.imm_f32() {
            ValueLocation::Immediate(
                wasmparser::Ieee32(f32::from_bits(i.bits()).abs().to_bits()).into(),
            )
        } else {
            let reg = self.into_temp_reg(GPRType::Rx, val);
            let const_label = self.abs_const_f32_label();

            dynasm!(self.asm
                ; andps Rx(reg.rx().unwrap()), [=>const_label.0]
            );

            ValueLocation::Reg(reg)
        };

        self.push(out);
    }

    pub fn f64_abs(&mut self) {
        let val = self.pop();

        let out = if let Some(i) = val.imm_f64() {
            ValueLocation::Immediate(
                wasmparser::Ieee64(f64::from_bits(i.bits()).abs().to_bits()).into(),
            )
        } else {
            let reg = self.into_temp_reg(GPRType::Rx, val);
            let const_label = self.abs_const_f64_label();

            dynasm!(self.asm
                ; andps Rx(reg.rx().unwrap()), [=>const_label.0]
            );

            ValueLocation::Reg(reg)
        };

        self.push(out);
    }

    unop!(i32_clz, lzcnt, Rd, u32, u32::leading_zeros);
    unop!(i64_clz, lzcnt, Rq, u64, |a: u64| a.leading_zeros() as u64);
    unop!(i32_ctz, tzcnt, Rd, u32, u32::trailing_zeros);
    unop!(i64_ctz, tzcnt, Rq, u64, |a: u64| a.trailing_zeros() as u64);

    pub fn i32_extend_u(&mut self) {
        let val = self.pop();

        self.free_value(val);
        let new_reg = self.block_state.regs.take(I64);

        let out = if let ValueLocation::Immediate(imm) = val {
            self.block_state.regs.release(new_reg);
            ValueLocation::Immediate((imm.as_i32().unwrap() as u32 as u64).into())
        } else {
            match val {
                ValueLocation::Reg(GPR::Rx(rxreg)) => {
                    dynasm!(self.asm
                        ; movd Rd(new_reg.rq().unwrap()), Rx(rxreg)
                    );
                }
                ValueLocation::Reg(GPR::Rq(rqreg)) => {
                    dynasm!(self.asm
                        ; mov Rd(new_reg.rq().unwrap()), Rd(rqreg)
                    );
                }
                ValueLocation::Stack(offset) => {
                    let offset = self.adjusted_offset(offset);

                    dynasm!(self.asm
                        ; mov Rd(new_reg.rq().unwrap()), [rsp + offset]
                    );
                }
                _ => unreachable!(),
            }

            ValueLocation::Reg(new_reg)
        };

        self.push(out);
    }

    pub fn i32_extend_s(&mut self) {
        let val = self.pop();

        self.free_value(val);
        let new_reg = self.block_state.regs.take(I64);

        let out = if let ValueLocation::Immediate(imm) = val {
            self.block_state.regs.release(new_reg);
            ValueLocation::Immediate((imm.as_i32().unwrap() as i64).into())
        } else {
            match val {
                ValueLocation::Reg(GPR::Rx(rxreg)) => {
                    dynasm!(self.asm
                        ; movd Rd(new_reg.rq().unwrap()), Rx(rxreg)
                        ; movsxd Rq(new_reg.rq().unwrap()), Rd(new_reg.rq().unwrap())
                    );
                }
                ValueLocation::Reg(GPR::Rq(rqreg)) => {
                    dynasm!(self.asm
                        ; movsxd Rq(new_reg.rq().unwrap()), Rd(rqreg)
                    );
                }
                ValueLocation::Stack(offset) => {
                    let offset = self.adjusted_offset(offset);

                    dynasm!(self.asm
                        ; movsxd Rq(new_reg.rq().unwrap()), DWORD [rsp + offset]
                    );
                }
                _ => unreachable!(),
            }

            ValueLocation::Reg(new_reg)
        };

        self.push(out);
    }

    unop!(i32_popcnt, popcnt, Rd, u32, u32::count_ones);
    conversion!(
        i32_truncate_f32,
        cvttss2si,
        Rx,
        rx,
        Rd,
        rq,
        f32,
        i32,
        as_f32,
        |a: wasmparser::Ieee32| a.bits()
    );
    conversion!(
        f32_convert_from_i32_s,
        cvtsi2ss,
        Rd,
        rq,
        Rx,
        rx,
        i32,
        f32,
        as_i32,
        |a| wasmparser::Ieee32((a as f32).to_bits())
    );

    pub fn f32_convert_from_i32_u(&mut self) {
        let mut val = self.pop();

        let out_val = match val {
            ValueLocation::Immediate(imm) => ValueLocation::Immediate(
                wasmparser::Ieee32((imm.as_i32().unwrap() as u32 as f32).to_bits()).into(),
            ),
            _ => {
                let reg = self.into_reg(I32, val);
                val = ValueLocation::Reg(reg);

                let temp = self.block_state.regs.take(F32);

                dynasm!(self.asm
                    ; mov Rq(reg.rq().unwrap()), Rq(reg.rq().unwrap())
                    ; cvtsi2ss Rx(temp.rx().unwrap()), Rq(reg.rq().unwrap())
                );

                ValueLocation::Reg(temp)
            }
        };

        self.free_value(val);

        self.push(out_val);
    }

    unop!(i64_popcnt, popcnt, Rq, u64, |a: u64| a.count_ones() as u64);

    // TODO: Use `lea` when the LHS operand isn't a temporary but both of the operands
    //       are in registers.
    commutative_binop_i32!(i32_add, add, i32::wrapping_add);
    commutative_binop_i32!(i32_and, and, |a, b| a & b);
    commutative_binop_i32!(i32_or, or, |a, b| a | b);
    commutative_binop_i32!(i32_xor, xor, |a, b| a ^ b);
    binop_i32!(i32_sub, sub, i32::wrapping_sub);

    commutative_binop_i64!(i64_add, add, i64::wrapping_add);
    commutative_binop_i64!(i64_and, and, |a, b| a & b);
    commutative_binop_i64!(i64_or, or, |a, b| a | b);
    commutative_binop_i64!(i64_xor, xor, |a, b| a ^ b);
    binop_i64!(i64_sub, sub, i64::wrapping_sub);

    commutative_binop_f32!(f32_add, addss, |a, b| a + b);
    commutative_binop_f32!(f32_mul, mulss, |a, b| a * b);
    binop_f32!(f32_sub, subss, |a, b| a - b);
    binop_f32!(f32_div, divss, |a, b| a / b);

    commutative_binop_f64!(f64_add, addsd, |a, b| a + b);
    commutative_binop_f64!(f64_mul, mulsd, |a, b| a * b);
    binop_f64!(f64_sub, subsd, |a, b| a - b);
    binop_f64!(f64_div, divsd, |a, b| a / b);

    shift!(
        i32_shl,
        Rd,
        shl,
        |a, b| (a as i32).wrapping_shl(b as _),
        I32
    );
    shift!(
        i32_shr_s,
        Rd,
        sar,
        |a, b| (a as i32).wrapping_shr(b as _),
        I32
    );
    shift!(
        i32_shr_u,
        Rd,
        shr,
        |a, b| (a as u32).wrapping_shr(b as _),
        I32
    );
    shift!(
        i32_rotl,
        Rd,
        rol,
        |a, b| (a as u32).rotate_left(b as _),
        I32
    );
    shift!(
        i32_rotr,
        Rd,
        ror,
        |a, b| (a as u32).rotate_right(b as _),
        I32
    );

    shift!(
        i64_shl,
        Rq,
        shl,
        |a, b| (a as i64).wrapping_shl(b as _),
        I64
    );
    shift!(
        i64_shr_s,
        Rq,
        sar,
        |a, b| (a as i64).wrapping_shr(b as _),
        I64
    );
    shift!(
        i64_shr_u,
        Rq,
        shr,
        |a, b| (a as u64).wrapping_shr(b as _),
        I64
    );
    shift!(
        i64_rotl,
        Rq,
        rol,
        |a, b| (a as u64).rotate_left(b as _),
        I64
    );
    shift!(
        i64_rotr,
        Rq,
        ror,
        |a, b| (a as u64).rotate_right(b as _),
        I64
    );

    fn cleanup_gprs(&mut self, gprs: impl Iterator<Item = (GPR, GPR)>) {
        for (src, dst) in gprs {
            self.copy_value(&ValueLocation::Reg(src), &mut ValueLocation::Reg(dst));
            self.block_state.regs.release(src);
            self.block_state.regs.mark_used(dst);
        }
    }

    int_div!(
        i32_full_div_s,
        i32_full_div_u,
        i32_div_u,
        i32_div_s,
        i32_rem_u,
        i32_rem_s,
        imm_i32,
        i32,
        u32
    );
    int_div!(
        i64_full_div_s,
        i64_full_div_u,
        i64_div_u,
        i64_div_s,
        i64_rem_u,
        i64_rem_s,
        imm_i64,
        i64,
        u64
    );

    /// Returned divisor is guaranteed not to be `RAX`
    // TODO: With a proper SSE-like "Value" system we could do this way better (we wouldn't have
    //       to move `RAX` back afterwards).
    fn full_div(
        &mut self,
        divisor: ValueLocation,
        quotient: ValueLocation,
        do_div: impl FnOnce(&mut Self, ValueLocation),
    ) -> (
        ValueLocation,
        ValueLocation,
        impl Iterator<Item = (GPR, GPR)> + Clone,
    ) {
        self.block_state.regs.mark_used(RAX);
        self.block_state.regs.mark_used(RDX);
        let divisor = if divisor == ValueLocation::Reg(RAX) || divisor == ValueLocation::Reg(RDX) {
            let new_reg = self.block_state.regs.take(I32);
            self.copy_value(&divisor, &mut ValueLocation::Reg(new_reg));
            self.free_value(divisor);
            ValueLocation::Reg(new_reg)
        } else if let ValueLocation::Stack(_) = divisor {
            divisor
        } else {
            ValueLocation::Reg(self.into_reg(I32, divisor))
        };
        self.block_state.regs.release(RDX);
        self.block_state.regs.release(RAX);

        if let ValueLocation::Reg(r) = quotient {
            self.block_state.regs.mark_used(r);
        }

        let should_save_rax =
            quotient != ValueLocation::Reg(RAX) && !self.block_state.regs.is_free(RAX);

        let saved_rax = if should_save_rax {
            let new_reg = self.block_state.regs.take(I32);
            dynasm!(self.asm
                ; mov Rq(new_reg.rq().unwrap()), rax
            );
            Some(new_reg)
        } else {
            None
        };

        self.block_state.regs.mark_used(RAX);
        self.copy_value(&quotient, &mut ValueLocation::Reg(RAX));
        self.free_value(quotient);

        let should_save_rdx = !self.block_state.regs.is_free(RDX);

        let saved_rdx = if should_save_rdx {
            let new_reg = self.block_state.regs.take(I32);
            dynasm!(self.asm
                ; mov Rq(new_reg.rq().unwrap()), rdx
            );
            Some(new_reg)
        } else {
            None
        };

        do_div(self, divisor);

        self.free_value(divisor);
        self.block_state.regs.mark_used(RDX);

        (
            ValueLocation::Reg(RAX),
            ValueLocation::Reg(RDX),
            saved_rax
                .map(|s| (s, RAX))
                .into_iter()
                .chain(saved_rdx.map(|s| (s, RDX))),
        )
    }

    fn i32_full_div_u(
        &mut self,
        divisor: ValueLocation,
        quotient: ValueLocation,
    ) -> (
        ValueLocation,
        ValueLocation,
        impl Iterator<Item = (GPR, GPR)> + Clone + 'module,
    ) {
        self.full_div(divisor, quotient, |this, divisor| match divisor {
            ValueLocation::Stack(offset) => {
                let offset = this.adjusted_offset(offset);
                dynasm!(this.asm
                    ; xor edx, edx
                    ; div DWORD [rsp + offset]
                );
            }
            ValueLocation::Reg(r) => {
                dynasm!(this.asm
                    ; xor edx, edx
                    ; div Rd(r.rq().unwrap())
                );
            }
            ValueLocation::Immediate(_) => unreachable!(),
        })
    }

    fn i32_full_div_s(
        &mut self,
        divisor: ValueLocation,
        quotient: ValueLocation,
    ) -> (
        ValueLocation,
        ValueLocation,
        impl Iterator<Item = (GPR, GPR)> + Clone + 'module,
    ) {
        self.full_div(divisor, quotient, |this, divisor| match divisor {
            ValueLocation::Stack(offset) => {
                let offset = this.adjusted_offset(offset);
                dynasm!(this.asm
                    ; cdq
                    ; idiv DWORD [rsp + offset]
                );
            }
            ValueLocation::Reg(r) => {
                dynasm!(this.asm
                    ; cdq
                    ; idiv Rd(r.rq().unwrap())
                );
            }
            ValueLocation::Immediate(_) => unreachable!(),
        })
    }

    fn i64_full_div_u(
        &mut self,
        divisor: ValueLocation,
        quotient: ValueLocation,
    ) -> (
        ValueLocation,
        ValueLocation,
        impl Iterator<Item = (GPR, GPR)> + Clone + 'module,
    ) {
        self.full_div(divisor, quotient, |this, divisor| match divisor {
            ValueLocation::Stack(offset) => {
                let offset = this.adjusted_offset(offset);
                dynasm!(this.asm
                    ; xor rdx, rdx
                    ; div QWORD [rsp + offset]
                );
            }
            ValueLocation::Reg(r) => {
                dynasm!(this.asm
                    ; xor rdx, rdx
                    ; div Rq(r.rq().unwrap())
                );
            }
            ValueLocation::Immediate(_) => unreachable!(),
        })
    }

    fn i64_full_div_s(
        &mut self,
        divisor: ValueLocation,
        quotient: ValueLocation,
    ) -> (
        ValueLocation,
        ValueLocation,
        impl Iterator<Item = (GPR, GPR)> + Clone + 'module,
    ) {
        self.full_div(divisor, quotient, |this, divisor| match divisor {
            ValueLocation::Stack(offset) => {
                let offset = this.adjusted_offset(offset);
                dynasm!(this.asm
                    ; cqo
                    ; idiv QWORD [rsp + offset]
                );
            }
            ValueLocation::Reg(r) => {
                dynasm!(this.asm
                    ; cqo
                    ; idiv Rq(r.rq().unwrap())
                );
            }
            ValueLocation::Immediate(_) => unreachable!(),
        })
    }

    // `i32_mul` needs to be separate because the immediate form of the instruction
    // has a different syntax to the immediate form of the other instructions.
    pub fn i32_mul(&mut self) {
        let right = self.pop();
        let left = self.pop();

        if let Some(right) = right.immediate() {
            if let Some(left) = left.immediate() {
                self.push(ValueLocation::Immediate(
                    i32::wrapping_mul(right.as_i32().unwrap(), left.as_i32().unwrap()).into(),
                ));
                return;
            }
        }

        let (left, right) = match left {
            ValueLocation::Reg(_) => (left, right),
            _ => {
                if right.immediate().is_some() {
                    (left, right)
                } else {
                    (right, left)
                }
            }
        };

        let out = match right {
            ValueLocation::Reg(reg) => {
                let left = self.into_temp_reg(I32, left);
                dynasm!(self.asm
                    ; imul Rd(left.rq().unwrap()), Rd(reg.rq().unwrap())
                );
                left
            }
            ValueLocation::Stack(offset) => {
                let offset = self.adjusted_offset(offset);

                let left = self.into_temp_reg(I32, left);
                dynasm!(self.asm
                    ; imul Rd(left.rq().unwrap()), [rsp + offset]
                );
                left
            }
            ValueLocation::Immediate(i) => {
                let left = self.into_reg(I32, left);
                self.block_state.regs.release(left);
                let new_reg = self.block_state.regs.take(I32);
                dynasm!(self.asm
                    ; imul Rd(new_reg.rq().unwrap()), Rd(left.rq().unwrap()), i.as_i32().unwrap()
                );
                new_reg
            }
        };

        self.push(ValueLocation::Reg(out));
        self.free_value(right);
    }

    // `i64_mul` needs to be separate because the immediate form of the instruction
    // has a different syntax to the immediate form of the other instructions.
    pub fn i64_mul(&mut self) {
        let right = self.pop();
        let left = self.pop();

        if let Some(right) = right.immediate() {
            if let Some(left) = left.immediate() {
                self.push(ValueLocation::Immediate(
                    i64::wrapping_mul(right.as_i64().unwrap(), left.as_i64().unwrap()).into(),
                ));
                return;
            }
        }

        let (left, right) = match left {
            ValueLocation::Reg(_) => (left, right),
            _ => {
                if right.immediate().is_some() {
                    (left, right)
                } else {
                    (right, left)
                }
            }
        };

        let out = match right {
            ValueLocation::Reg(reg) => {
                let left = self.into_temp_reg(I64, left);
                dynasm!(self.asm
                    ; imul Rq(left.rq().unwrap()), Rq(reg.rq().unwrap())
                );
                left
            }
            ValueLocation::Stack(offset) => {
                let offset = self.adjusted_offset(offset);

                let left = self.into_temp_reg(I64, left);
                dynasm!(self.asm
                    ; imul Rq(left.rq().unwrap()), [rsp + offset]
                );
                left
            }
            ValueLocation::Immediate(i) => {
                let left = self.into_reg(I64, left);
                self.block_state.regs.release(left);
                let new_reg = self.block_state.regs.take(I64);

                let i = i.as_i64().unwrap();
                if let Some(i) = i.try_into() {
                    dynasm!(self.asm
                        ; imul Rq(new_reg.rq().unwrap()), Rq(left.rq().unwrap()), i
                    );
                } else {
                    unimplemented!();
                }

                new_reg
            }
        };

        self.push(ValueLocation::Reg(out));
        self.free_value(right);
    }

    pub fn select(&mut self) {
        let cond = self.pop();
        let else_ = self.pop();
        let then = self.pop();

        match cond {
            ValueLocation::Immediate(i) => {
                if i.as_i32().unwrap() == 0 {
                    self.push(else_);
                } else {
                    self.push(then);
                }

                return;
            }
            other => {
                let reg = self.into_reg(I32, other);

                dynasm!(self.asm
                    ; test Rd(reg.rq().unwrap()), Rd(reg.rq().unwrap())
                );

                self.block_state.regs.release(reg);
            }
        }

        let out_gpr = self.block_state.regs.take(GPRType::Rq);

        // TODO: Can do this better for variables on stack
        macro_rules! cmov_helper {
            ($instr:ident, $val:expr) => {
                match $val {
                    ValueLocation::Stack(offset) => {
                        let offset = self.adjusted_offset(offset);
                        dynasm!(self.asm
                            ; $instr Rq(out_gpr.rq().unwrap()), [rsp + offset]
                        );
                    }
                    other => {
                        let scratch = self.into_reg(GPRType::Rq, other);
                        dynasm!(self.asm
                            ; $instr Rq(out_gpr.rq().unwrap()), Rq(scratch.rq().unwrap())
                        );
                        self.block_state.regs.release(scratch);
                    }
                }
            }
        }

        cmov_helper!(cmovz, else_);
        cmov_helper!(cmovnz, then);

        self.push(ValueLocation::Reg(out_gpr));
    }

    pub fn pick(&mut self, depth: u32) {
        let idx = self.block_state.stack.len() - 1 - depth as usize;
        let v = self.block_state.stack[idx];

        match v {
            ValueLocation::Reg(r) => {
                self.block_state.regs.mark_used(r);
            }
            _ => {}
        }

        self.block_state.stack.push(v);
    }

    pub fn const_(&mut self, imm: Value) {
        self.push(ValueLocation::Immediate(imm));
    }

    fn relocated_function_call(
        &mut self,
        name: &cranelift_codegen::ir::ExternalName,
        args: impl IntoIterator<Item = SignlessType>,
        rets: impl IntoIterator<Item = SignlessType>,
    ) {
        self.pass_outgoing_args(&arg_locs(args));
        // 2 bytes for the 64-bit `mov` opcode + register ident, the rest is the immediate
        self.reloc_sink.reloc_external(
            (self.asm.offset().0
                - self.func_starts[self.current_function as usize]
                    .0
                    .unwrap()
                    .0) as u32
                + 2,
            binemit::Reloc::Abs8,
            name,
            0,
        );
        let temp = self.block_state.regs.take(I64);
        dynasm!(self.asm
            ; mov Rq(temp.rq().unwrap()), QWORD 0xdeadbeefdeadbeefu64 as i64
            ; call Rq(temp.rq().unwrap())
        );
        self.block_state.regs.release(temp);
        self.push_function_returns(rets);
    }

    // TODO: Other memory indices
    pub fn memory_size(&mut self) {
        self.push(ValueLocation::Immediate(0u32.into()));
        self.relocated_function_call(
            &magic::get_memory32_size_name(),
            iter::once(I32),
            iter::once(I32),
        );
        // let tmp = self.block_state.regs.take(I32);
        //
        // // 16 is log2(64KiB as bytes)
        // dynasm!(self.asm
        // ; mov Rd(tmp.rq().unwrap()), [
        // rdi + self.module_context.vmctx_vmmemory_definition_current_length(0) as i32
        // ]
        // ; shr Rd(tmp.rq().unwrap()), 16
        // );
        //
        // self.push(ValueLocation::Reg(tmp));
    }

    // TODO: Other memory indices
    pub fn memory_grow(&mut self) {
        self.push(ValueLocation::Immediate(0u32.into()));
        self.relocated_function_call(
            &magic::get_memory32_grow_name(),
            iter::once(I32).chain(iter::once(I32)),
            iter::once(I32),
        );
    }

    // TODO: Use `ArrayVec`?
    // TODO: This inefficiently duplicates registers but it's not really possible
    //       to double up stack space right now.
    /// Saves volatile (i.e. caller-saved) registers before a function call, if they are used.
    fn save_volatile(&mut self, bounds: impl std::ops::RangeBounds<usize>) {
        self.save_regs(SCRATCH_REGS, bounds);
    }

    fn save_regs<I>(&mut self, regs: &I, bounds: impl std::ops::RangeBounds<usize>)
    where
        for<'a> &'a I: IntoIterator<Item = &'a GPR>,
        I: ?Sized,
    {
        use std::ops::Bound::*;

        let mut stack = mem::replace(&mut self.block_state.stack, vec![]);
        let (start, end) = (
            match bounds.end_bound() {
                Unbounded => 0,
                Included(v) => stack.len() - 1 - v,
                Excluded(v) => stack.len() - v,
            },
            match bounds.start_bound() {
                Unbounded => stack.len(),
                Included(v) => stack.len() - v,
                Excluded(v) => stack.len() - 1 - v,
            },
        );
        for val in stack[start..end].iter_mut() {
            if let ValueLocation::Reg(vreg) = *val {
                if regs.into_iter().any(|r| *r == vreg) {
                    *val = self.push_physical(*val);
                }
            }
        }

        mem::replace(&mut self.block_state.stack, stack);
    }

    /// Write the arguments to the callee to the registers and the stack using the SystemV
    /// calling convention.
    fn pass_outgoing_args(&mut self, out_locs: &[CCLoc]) {
        self.save_volatile(out_locs.len()..);

        // TODO: Do alignment here
        let total_stack_space = out_locs
            .iter()
            .flat_map(|&l| {
                if let CCLoc::Stack(offset) = l {
                    if offset > 0 {
                        Some(offset as u32)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(0);
        let depth = self.block_state.depth.0 + total_stack_space;

        let mut pending = Vec::<(ValueLocation, ValueLocation)>::new();

        for &loc in out_locs.iter().rev() {
            let val = self.pop();

            match loc {
                CCLoc::Stack(offset) => {
                    let offset = self.adjusted_offset(offset as i32 - depth as i32);

                    if offset == -(WORD_SIZE as i32) {
                        self.push_physical(val);
                    } else {
                        let gpr = self.into_reg(GPRType::Rq, val);
                        dynasm!(self.asm
                            ; mov [rsp + offset], Rq(gpr.rq().unwrap())
                        );
                        self.block_state.regs.release(gpr);
                    }
                }
                CCLoc::Reg(r) => {
                    if val == ValueLocation::Reg(r) {
                        self.free_value(val);
                    } else if self.block_state.regs.is_free(r) {
                        self.copy_value(&val, &mut loc.into());
                        self.free_value(val);
                    } else {
                        pending.push((val, loc.into()));
                    }
                }
            }
        }

        let mut try_count = 10;
        while !pending.is_empty() {
            try_count -= 1;

            if try_count == 0 {
                unimplemented!("We can't handle cycles in the register allocation right now");
            }

            for (src, dst) in mem::replace(&mut pending, vec![]) {
                if let ValueLocation::Reg(r) = dst {
                    if !self.block_state.regs.is_free(r) {
                        pending.push((src, dst));
                        continue;
                    }
                }
                self.copy_value(&src, &mut { dst });
                self.free_value(src);
            }
        }

        self.set_stack_depth(StackDepth(depth));
    }

    // TODO: Multiple returns
    fn push_function_returns(&mut self, returns: impl IntoIterator<Item = SignlessType>) {
        for loc in ret_locs(returns) {
            if let CCLoc::Reg(reg) = loc {
                self.block_state.regs.mark_used(reg);
            }

            self.push(loc.into());
        }
    }

    // TODO: Do return types properly
    pub fn call_indirect(
        &mut self,
        type_id: u32,
        arg_types: impl IntoIterator<Item = SignlessType>,
        return_types: impl IntoIterator<Item = SignlessType>,
    ) {
        let locs = arg_locs(arg_types);

        for &loc in &locs {
            if let CCLoc::Reg(r) = loc {
                self.block_state.regs.mark_used(r);
            }
        }

        let callee = self.pop();
        let callee = self.into_temp_reg(I32, callee);
        let temp0 = self.block_state.regs.take(I64);
        let temp1 = self.block_state.regs.take(I64);

        for &loc in &locs {
            if let CCLoc::Reg(r) = loc {
                self.block_state.regs.release(r);
            }
        }

        self.pass_outgoing_args(&locs);

        let fail = self.trap_label().0;

        // TODO: Consider generating a single trap function and jumping to that instead.
        dynasm!(self.asm
            ; cmp Rd(callee.rq().unwrap()), [
                Rq(VMCTX) +
                    self.module_context
                        .vmctx_vmtable_definition_current_elements(0) as i32
            ]
            ; jae =>fail
            ; imul
                Rd(callee.rq().unwrap()),
                Rd(callee.rq().unwrap()),
                self.module_context.size_of_vmcaller_checked_anyfunc() as i32
            ; mov Rq(temp0.rq().unwrap()), [
                Rq(VMCTX) +
                    self.module_context
                        .vmctx_vmtable_definition_base(0) as i32
            ]
            ; mov Rd(temp1.rq().unwrap()), [
                Rq(VMCTX) +
                    self.module_context
                        .vmctx_vmshared_signature_id(type_id) as i32
            ]
            ; cmp DWORD [
                Rq(temp0.rq().unwrap()) +
                    Rq(callee.rq().unwrap()) +
                    self.module_context.vmcaller_checked_anyfunc_type_index() as i32
            ], Rd(temp1.rq().unwrap())
            ; jne =>fail
            ; call QWORD [
                Rq(temp0.rq().unwrap()) +
                    Rq(callee.rq().unwrap()) +
                    self.module_context.vmcaller_checked_anyfunc_func_ptr() as i32
            ]
        );

        self.block_state.regs.release(temp0);
        self.block_state.regs.release(temp1);
        self.block_state.regs.release(callee);

        self.push_function_returns(return_types);
    }

    pub fn swap(&mut self, depth: u32) {
        let last = self.block_state.stack.len() - 1;
        self.block_state.stack.swap(last, last - depth as usize);
    }

    /// Call a function with the given index
    pub fn call_direct(
        &mut self,
        index: u32,
        arg_types: impl IntoIterator<Item = SignlessType>,
        return_types: impl IntoIterator<Item = SignlessType>,
    ) {
        self.pass_outgoing_args(&arg_locs(arg_types));

        let label = &self.func_starts[index as usize].1;
        dynasm!(self.asm
            ; call =>*label
        );

        self.push_function_returns(return_types);
    }

    // TODO: Reserve space to store RBX, RBP, and R12..R15 so we can use them
    //       as scratch registers
    // TODO: Allow use of unused argument registers as scratch registers.
    /// Writes the function prologue and stores the arguments as locals
    pub fn start_function(&mut self, params: impl IntoIterator<Item = SignlessType>) {
        let locs = Vec::from_iter(arg_locs(params));
        self.apply_cc(&CallingConvention::function_start(locs));
    }

    pub fn ret(&mut self) {
        dynasm!(self.asm
            ; ret
        );
    }

    fn align(&mut self, align_to: u32) {
        dynasm!(self.asm
            ; .align align_to as usize
        );
    }

    /// Writes the function epilogue (right now all this does is add the trap label that the
    /// conditional traps in `call_indirect` use)
    pub fn epilogue(&mut self) {
        // TODO: We don't want to redefine this label if we're sharing it between functions
        if let Some(l) = self
            .labels
            .trap
            .as_ref()
            .and_then(PendingLabel::as_undefined)
        {
            self.define_label(l);
            dynasm!(self.asm
                ; ud2
            );
            self.labels.trap = Some(PendingLabel::defined(l));
        }

        if let Some(l) = self
            .labels
            .ret
            .as_ref()
            .and_then(PendingLabel::as_undefined)
        {
            self.define_label(l);
            dynasm!(self.asm
                ; ret
            );
            self.labels.ret = Some(PendingLabel::defined(l));
        }

        if let Some(l) = self
            .labels
            .neg_const_f32
            .as_ref()
            .and_then(PendingLabel::as_undefined)
        {
            self.align(16);
            self.define_label(l);
            dynasm!(self.asm
                ; .dword -2147483648
                ; .dword 0
                ; .dword 0
                ; .dword 0
            );
            self.labels.neg_const_f32 = Some(PendingLabel::defined(l));
        }

        if let Some(l) = self
            .labels
            .neg_const_f64
            .as_ref()
            .and_then(PendingLabel::as_undefined)
        {
            self.align(16);
            self.define_label(l);
            dynasm!(self.asm
                ; .dword 0
                ; .dword -2147483648
                ; .dword 0
                ; .dword 0
            );
            self.labels.neg_const_f64 = Some(PendingLabel::defined(l));
        }

        if let Some(l) = self
            .labels
            .abs_const_f32
            .as_ref()
            .and_then(PendingLabel::as_undefined)
        {
            self.align(16);
            self.define_label(l);
            dynasm!(self.asm
                ; .dword 2147483647
                ; .dword 2147483647
                ; .dword 2147483647
                ; .dword 2147483647
            );
            self.labels.abs_const_f32 = Some(PendingLabel::defined(l));
        }

        if let Some(l) = self
            .labels
            .abs_const_f64
            .as_ref()
            .and_then(PendingLabel::as_undefined)
        {
            self.align(16);
            self.define_label(l);
            dynasm!(self.asm
                ; .qword 9223372036854775807
                ; .qword 9223372036854775807
            );
            self.labels.abs_const_f64 = Some(PendingLabel::defined(l));
        }
    }

    pub fn trap(&mut self) {
        dynasm!(self.asm
            ; ud2
        );
    }

    fn target_to_label(&mut self, target: BrTarget<Label>) -> Label {
        match target {
            BrTarget::Label(label) => label,
            BrTarget::Return => self.ret_label(),
        }
    }

    #[must_use]
    fn trap_label(&mut self) -> Label {
        if let Some(l) = &self.labels.trap {
            return l.label;
        }

        let label = self.create_label();
        self.labels.trap = Some(label.into());
        label
    }

    #[must_use]
    fn ret_label(&mut self) -> Label {
        if let Some(l) = &self.labels.ret {
            return l.label;
        }

        let label = self.create_label();
        self.labels.ret = Some(label.into());
        label
    }

    #[must_use]
    fn neg_const_f32_label(&mut self) -> Label {
        if let Some(l) = &self.labels.neg_const_f32 {
            return l.label;
        }

        let label = self.create_label();
        self.labels.neg_const_f32 = Some(label.into());
        label
    }

    #[must_use]
    fn neg_const_f64_label(&mut self) -> Label {
        if let Some(l) = &self.labels.neg_const_f64 {
            return l.label;
        }

        let label = self.create_label();
        self.labels.neg_const_f64 = Some(label.into());
        label
    }

    #[must_use]
    fn abs_const_f32_label(&mut self) -> Label {
        if let Some(l) = &self.labels.abs_const_f32 {
            return l.label;
        }

        let label = self.create_label();
        self.labels.abs_const_f32 = Some(label.into());
        label
    }

    #[must_use]
    fn abs_const_f64_label(&mut self) -> Label {
        if let Some(l) = &self.labels.abs_const_f64 {
            return l.label;
        }

        let label = self.create_label();
        self.labels.abs_const_f64 = Some(label.into());
        label
    }
}

