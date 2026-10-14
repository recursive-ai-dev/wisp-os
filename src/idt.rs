//! Our own Interrupt Descriptor Table: every one of the 256 vectors gets a
//! real, hand-written entry point -- 32 CPU exception handlers and 224
//! interrupt handlers -- rather than leaving unused vectors absent (which
//! would turn an unexpected fault into a second, harder-to-diagnose fault:
//! "IDT entry not present," #GP, right on top of whatever actually went
//! wrong).
//!
//! # Why hand-written `#[unsafe(naked)]` trampolines, not `extern
//! "x86-interrupt"`
//!
//! The nightly-only `x86-interrupt` calling convention would let a normal
//! Rust `fn` be used directly as an interrupt handler, with the compiler
//! generating the register save/restore and `iretq`. It has a history of
//! real codegen bugs (e.g. rust-lang/rust#109918) and is not on any
//! stabilization path. `#[unsafe(naked)]` + `naked_asm!` (stable since Rust
//! 1.88) instead puts every instruction of the entry/exit sequence in our
//! own hands, in the same spirit as everything else in this kernel.
//!
//! # Trampoline shapes
//!
//! Every vector needs its own tiny entry point (the IDT gate stores a
//! fixed address; there is no way for one shared entry point to know which
//! vector invoked it unless something at that address encodes it), so this
//! file defines 256 near-identical stubs via three small macros rather than
//! writing each out by hand. Two shapes:
//!
//! - **Fault stubs** (vectors 0-31, CPU exceptions): normalize the stack
//!   so every vector looks the same regardless of whether the CPU itself
//!   pushed a hardware error code (10 of the 32 do; see `ERR_VECS` below)
//!   by pushing a dummy 0 for the ones that don't, save every general
//!   register for the fault screen's register dump, and call
//!   [`fault_handler_entry`] -- which never returns. `ud2` after the call
//!   is a belt-and-brace: it should be unreachable, and if it's ever
//!   reached (a change to `panic::fault_screen` that lets it return, say)
//!   it's better to take an immediate, obvious `#UD` than silently execute
//!   whatever garbage follows.
//! - **IRQ stubs** (vectors 32-255): same save sequence, but they call
//!   [`irq_handler_entry`], fully restore every register, and `iretq` --
//!   the machine must keep running after a timer tick or keystroke.
//!
//! # Stack layout, alignment, and the calling convention gotcha that bit us
//!
//! After a fault stub's pushes, the stack (low to high address) reads:
//! `r15, r14, ..., r8, rbp, rdi, rsi, rdx, rcx, rbx, rax, vector,
//! error_code, rip, cs, rflags, rsp, ss` -- exactly the field order of
//! [`InterruptFrame`] below, so `rcx = rsp` at the point of `call` is a
//! valid `*const InterruptFrame` -- **`rcx`, not `rdi`**. This target's
//! `extern "C"` is the Microsoft x64 calling convention (first integer/
//! pointer argument in RCX, not RDI as on SysV/Linux/Mac), because that's
//! what UEFI itself is specified against; every `efiapi`/`extern "C"` call
//! in this whole codebase already goes through it via the `uefi` crate.
//! The first version of this file got this wrong -- `mov rdi, rsp` instead
//! of `mov rcx, rsp` -- and the result was exactly as bad as it sounds:
//! `fault_handler_entry`/`irq_handler_entry` read their "frame pointer"
//! out of whatever garbage happened to be sitting in RCX, producing a
//! `frame` pointing at essentially a random low address and a triple fault
//! the instant a real interrupt arrived, with the CPU state actually
//! correct the entire time. Confirmed by disassembling the release binary
//! and comparing `mov %rsp,%rdi` in the stub against `mov 0x78(%rcx),%rsi`
//! (the compiled body of `irq_handler_entry` reading `frame.vector`) --
//! see `CHANGES.md` for the full account of diagnosing this.
//!
//! The Microsoft x64 ABI also requires the *caller* to reserve 32 bytes of
//! "shadow space" on the stack immediately below the return address before
//! every call (space the callee is entitled to spill its register
//! arguments into), in addition to the usual 16-byte stack alignment
//! requirement immediately before `call`. The x86_64 CPU itself guarantees
//! the stack is 16-byte aligned at the very first instruction of an
//! interrupt handler when no error code was pushed (and 8-off when one
//! was, since it's one extra qword) -- see Intel SDM Vol 3, 6.14.2. Our
//! own pushes bring both cases to a common 16-aligned state (one
//! dummy-or-real error code + one vector push = 16 bytes either way), but
//! the subsequent 15-register save (120 bytes, not a multiple of 16)
//! throws that off by 8 again. `sub rsp, 0x20` / `add rsp, 0x20`
//! bracketing the `call` restores 16-alignment *and* provides the required
//! 32-byte shadow space in one step (32 is itself a multiple of 16) --
//! the pointer handed to Rust is captured (into `rcx`) *before* the `sub`,
//! so that reserved region lives below (at higher addresses than) the
//! frame Rust reads and never disturbs it.

use core::arch::{asm, naked_asm};
use core::mem::size_of;

use crate::{debugcon, gdt, lapic, panic, pic};

/// The full saved CPU state at the moment a fault or interrupt was taken,
/// exactly matching the stub's push order (see module docs). Every field
/// is a plain `u64` because the corresponding push is always 8 bytes on
/// this target ('naked_asm push reg' is a 64-bit push in long mode).
#[repr(C)]
pub struct InterruptFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    /// Our own addition, not part of the hardware frame: the vector
    /// number, pushed by the stub itself so one common handler can tell
    /// which of the 256 gates fired.
    pub vector: u64,
    /// Real hardware error code for the 10 exceptions that have one (see
    /// `ERR_VECS`); a dummy 0 pushed by the stub for every other vector,
    /// so this field is always meaningful to read, never uninitialized
    /// stack garbage.
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

/// One 16-byte x86_64 IDT gate descriptor (Intel SDM Vol 3, 6.14.1).
#[derive(Clone, Copy)]
#[repr(C, packed)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    /// Low 3 bits: Interrupt Stack Table index (0 = don't switch stacks,
    /// 1-7 = TSS.IST[index-1]). Upper 5 bits reserved, must be 0.
    ist: u8,
    /// P(7) DPL(6:5) 0(4) Type(3:0). `0x8E` = present, ring 0, 64-bit
    /// interrupt gate (type 0b1110) -- an interrupt gate (as opposed to a
    /// trap gate, type 0b1111) clears RFLAGS.IF on entry, which is exactly
    /// what makes "ISRs can't be interrupted by another ISR" true on this
    /// single-core kernel without any explicit lock.
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const MISSING: IdtEntry = IdtEntry {
        offset_low: 0,
        selector: 0,
        ist: 0,
        type_attr: 0, // Present bit clear: any fault here is a real #GP, not silently wrong.
        offset_mid: 0,
        offset_high: 0,
        reserved: 0,
    };

    fn new(handler: u64, ist: u8) -> Self {
        IdtEntry {
            offset_low: (handler & 0xFFFF) as u16,
            selector: gdt::KERNEL_CODE_SELECTOR,
            ist: ist & 0x7,
            type_attr: 0x8E,
            offset_mid: ((handler >> 16) & 0xFFFF) as u16,
            offset_high: ((handler >> 32) & 0xFFFF_FFFF) as u32,
            reserved: 0,
        }
    }
}

#[repr(C, align(16))]
struct Idt([IdtEntry; 256]);

static mut IDT: Idt = Idt([IdtEntry::MISSING; 256]);

// --- Stub generation ----------------------------------------------------

macro_rules! fault_stub_noerr {
    ($($vec:literal => $name:ident),+ $(,)?) => {
        $(
            #[unsafe(naked)]
            unsafe extern "C" fn $name() -> ! {
                naked_asm!(
                    "push 0",       // dummy error code: this vector's exception has none
                    "push {vec}",
                    "push rax", "push rbx", "push rcx", "push rdx",
                    "push rsi", "push rdi", "push rbp",
                    "push r8", "push r9", "push r10", "push r11",
                    "push r12", "push r13", "push r14", "push r15",
                    "mov rcx, rsp",
                    "sub rsp, 0x20", // 32-byte MS x64 ABI shadow space (also keeps 16-alignment); see module docs
                    "call {handler}",
                    "ud2",          // unreachable: fault_handler_entry never returns
                    vec = const $vec,
                    handler = sym fault_handler_entry,
                )
            }
        )+
    };
}

macro_rules! fault_stub_err {
    ($($vec:literal => $name:ident),+ $(,)?) => {
        $(
            #[unsafe(naked)]
            unsafe extern "C" fn $name() -> ! {
                naked_asm!(
                    "push {vec}",   // hardware already pushed the real error code
                    "push rax", "push rbx", "push rcx", "push rdx",
                    "push rsi", "push rdi", "push rbp",
                    "push r8", "push r9", "push r10", "push r11",
                    "push r12", "push r13", "push r14", "push r15",
                    "mov rcx, rsp",
                    "sub rsp, 0x20",
                    "call {handler}",
                    "ud2",
                    vec = const $vec,
                    handler = sym fault_handler_entry,
                )
            }
        )+
    };
}

macro_rules! irq_stub {
    ($($vec:literal => $name:ident),+ $(,)?) => {
        $(
            #[unsafe(naked)]
            unsafe extern "C" fn $name() {
                naked_asm!(
                    "push 0",       // no external interrupt ever carries a hardware error code
                    "push {vec}",
                    "push rax", "push rbx", "push rcx", "push rdx",
                    "push rsi", "push rdi", "push rbp",
                    "push r8", "push r9", "push r10", "push r11",
                    "push r12", "push r13", "push r14", "push r15",
                    "mov rcx, rsp",
                    "sub rsp, 0x20",
                    "call {handler}",
                    "add rsp, 0x20",
                    "pop r15", "pop r14", "pop r13", "pop r12",
                    "pop r11", "pop r10", "pop r9", "pop r8",
                    "pop rbp", "pop rdi", "pop rsi", "pop rdx",
                    "pop rcx", "pop rbx", "pop rax",
                    "add rsp, 16",  // discard our vector + error-code slots
                    "iretq",
                    vec = const $vec,
                    handler = sym irq_handler_entry,
                )
            }
        )+
    };
}

// Vectors 0-31 are CPU exceptions. Ten of them push a real hardware error
// code (Intel SDM Vol 3, 6.15, Table 6-1); every other vector in 0-31 does
// not, and gets a dummy one from the stub so `InterruptFrame::error_code`
// is always valid to read.
fault_stub_noerr!(
    0 => isr_0, 1 => isr_1, 2 => isr_2, 3 => isr_3, 4 => isr_4, 5 => isr_5, 6 => isr_6, 7 => isr_7, 9 => isr_9, 15 => isr_15, 16 => isr_16, 18 => isr_18, 19 => isr_19, 20 => isr_20, 22 => isr_22, 23 => isr_23, 24 => isr_24, 25 => isr_25, 26 => isr_26, 27 => isr_27, 28 => isr_28, 31 => isr_31,
);

fault_stub_err!(
    8 => isr_8, 10 => isr_10, 11 => isr_11, 12 => isr_12, 13 => isr_13, 14 => isr_14, 17 => isr_17, 21 => isr_21, 29 => isr_29, 30 => isr_30,
);

// Vectors 32-255: 32-47 are the PIC-remapped hardware IRQ lines (see
// `pic.rs`); 48-255 are never triggered by anything on this system (no
// APIC, no software `int` instructions issued anywhere above 0x2F) but
// still get a real handler rather than an absent gate.
irq_stub!(
    32 => isr_32, 33 => isr_33, 34 => isr_34, 35 => isr_35, 36 => isr_36, 37 => isr_37,
    38 => isr_38, 39 => isr_39, 40 => isr_40, 41 => isr_41, 42 => isr_42, 43 => isr_43,
    44 => isr_44, 45 => isr_45, 46 => isr_46, 47 => isr_47, 48 => isr_48, 49 => isr_49,
    50 => isr_50, 51 => isr_51, 52 => isr_52, 53 => isr_53, 54 => isr_54, 55 => isr_55,
    56 => isr_56, 57 => isr_57, 58 => isr_58, 59 => isr_59, 60 => isr_60, 61 => isr_61,
    62 => isr_62, 63 => isr_63, 64 => isr_64, 65 => isr_65, 66 => isr_66, 67 => isr_67,
    68 => isr_68, 69 => isr_69, 70 => isr_70, 71 => isr_71, 72 => isr_72, 73 => isr_73,
    74 => isr_74, 75 => isr_75, 76 => isr_76, 77 => isr_77, 78 => isr_78, 79 => isr_79,
    80 => isr_80, 81 => isr_81, 82 => isr_82, 83 => isr_83, 84 => isr_84, 85 => isr_85,
    86 => isr_86, 87 => isr_87, 88 => isr_88, 89 => isr_89, 90 => isr_90, 91 => isr_91,
    92 => isr_92, 93 => isr_93, 94 => isr_94, 95 => isr_95, 96 => isr_96, 97 => isr_97,
    98 => isr_98, 99 => isr_99, 100 => isr_100, 101 => isr_101, 102 => isr_102, 103 => isr_103,
    104 => isr_104, 105 => isr_105, 106 => isr_106, 107 => isr_107, 108 => isr_108, 109 => isr_109,
    110 => isr_110, 111 => isr_111, 112 => isr_112, 113 => isr_113, 114 => isr_114, 115 => isr_115,
    116 => isr_116, 117 => isr_117, 118 => isr_118, 119 => isr_119, 120 => isr_120, 121 => isr_121,
    122 => isr_122, 123 => isr_123, 124 => isr_124, 125 => isr_125, 126 => isr_126, 127 => isr_127,
    128 => isr_128, 129 => isr_129, 130 => isr_130, 131 => isr_131, 132 => isr_132, 133 => isr_133,
    134 => isr_134, 135 => isr_135, 136 => isr_136, 137 => isr_137, 138 => isr_138, 139 => isr_139,
    140 => isr_140, 141 => isr_141, 142 => isr_142, 143 => isr_143, 144 => isr_144, 145 => isr_145,
    146 => isr_146, 147 => isr_147, 148 => isr_148, 149 => isr_149, 150 => isr_150, 151 => isr_151,
    152 => isr_152, 153 => isr_153, 154 => isr_154, 155 => isr_155, 156 => isr_156, 157 => isr_157,
    158 => isr_158, 159 => isr_159, 160 => isr_160, 161 => isr_161, 162 => isr_162, 163 => isr_163,
    164 => isr_164, 165 => isr_165, 166 => isr_166, 167 => isr_167, 168 => isr_168, 169 => isr_169,
    170 => isr_170, 171 => isr_171, 172 => isr_172, 173 => isr_173, 174 => isr_174, 175 => isr_175,
    176 => isr_176, 177 => isr_177, 178 => isr_178, 179 => isr_179, 180 => isr_180, 181 => isr_181,
    182 => isr_182, 183 => isr_183, 184 => isr_184, 185 => isr_185, 186 => isr_186, 187 => isr_187,
    188 => isr_188, 189 => isr_189, 190 => isr_190, 191 => isr_191, 192 => isr_192, 193 => isr_193,
    194 => isr_194, 195 => isr_195, 196 => isr_196, 197 => isr_197, 198 => isr_198, 199 => isr_199,
    200 => isr_200, 201 => isr_201, 202 => isr_202, 203 => isr_203, 204 => isr_204, 205 => isr_205,
    206 => isr_206, 207 => isr_207, 208 => isr_208, 209 => isr_209, 210 => isr_210, 211 => isr_211,
    212 => isr_212, 213 => isr_213, 214 => isr_214, 215 => isr_215, 216 => isr_216, 217 => isr_217,
    218 => isr_218, 219 => isr_219, 220 => isr_220, 221 => isr_221, 222 => isr_222, 223 => isr_223,
    224 => isr_224, 225 => isr_225, 226 => isr_226, 227 => isr_227, 228 => isr_228, 229 => isr_229,
    230 => isr_230, 231 => isr_231, 232 => isr_232, 233 => isr_233, 234 => isr_234, 235 => isr_235,
    236 => isr_236, 237 => isr_237, 238 => isr_238, 239 => isr_239, 240 => isr_240, 241 => isr_241,
    242 => isr_242, 243 => isr_243, 244 => isr_244, 245 => isr_245, 246 => isr_246, 247 => isr_247,
    248 => isr_248, 249 => isr_249, 250 => isr_250, 251 => isr_251, 252 => isr_252, 253 => isr_253,
    254 => isr_254, 255 => isr_255,
);

fn stub_addrs() -> [u64; 256] {
    [
    isr_0 as *const () as usize as u64, isr_1 as *const () as usize as u64, isr_2 as *const () as usize as u64, isr_3 as *const () as usize as u64,
    isr_4 as *const () as usize as u64, isr_5 as *const () as usize as u64, isr_6 as *const () as usize as u64, isr_7 as *const () as usize as u64,
    isr_8 as *const () as usize as u64, isr_9 as *const () as usize as u64, isr_10 as *const () as usize as u64, isr_11 as *const () as usize as u64,
    isr_12 as *const () as usize as u64, isr_13 as *const () as usize as u64, isr_14 as *const () as usize as u64, isr_15 as *const () as usize as u64,
    isr_16 as *const () as usize as u64, isr_17 as *const () as usize as u64, isr_18 as *const () as usize as u64, isr_19 as *const () as usize as u64,
    isr_20 as *const () as usize as u64, isr_21 as *const () as usize as u64, isr_22 as *const () as usize as u64, isr_23 as *const () as usize as u64,
    isr_24 as *const () as usize as u64, isr_25 as *const () as usize as u64, isr_26 as *const () as usize as u64, isr_27 as *const () as usize as u64,
    isr_28 as *const () as usize as u64, isr_29 as *const () as usize as u64, isr_30 as *const () as usize as u64, isr_31 as *const () as usize as u64,
    isr_32 as *const () as usize as u64, isr_33 as *const () as usize as u64, isr_34 as *const () as usize as u64, isr_35 as *const () as usize as u64,
    isr_36 as *const () as usize as u64, isr_37 as *const () as usize as u64, isr_38 as *const () as usize as u64, isr_39 as *const () as usize as u64,
    isr_40 as *const () as usize as u64, isr_41 as *const () as usize as u64, isr_42 as *const () as usize as u64, isr_43 as *const () as usize as u64,
    isr_44 as *const () as usize as u64, isr_45 as *const () as usize as u64, isr_46 as *const () as usize as u64, isr_47 as *const () as usize as u64,
    isr_48 as *const () as usize as u64, isr_49 as *const () as usize as u64, isr_50 as *const () as usize as u64, isr_51 as *const () as usize as u64,
    isr_52 as *const () as usize as u64, isr_53 as *const () as usize as u64, isr_54 as *const () as usize as u64, isr_55 as *const () as usize as u64,
    isr_56 as *const () as usize as u64, isr_57 as *const () as usize as u64, isr_58 as *const () as usize as u64, isr_59 as *const () as usize as u64,
    isr_60 as *const () as usize as u64, isr_61 as *const () as usize as u64, isr_62 as *const () as usize as u64, isr_63 as *const () as usize as u64,
    isr_64 as *const () as usize as u64, isr_65 as *const () as usize as u64, isr_66 as *const () as usize as u64, isr_67 as *const () as usize as u64,
    isr_68 as *const () as usize as u64, isr_69 as *const () as usize as u64, isr_70 as *const () as usize as u64, isr_71 as *const () as usize as u64,
    isr_72 as *const () as usize as u64, isr_73 as *const () as usize as u64, isr_74 as *const () as usize as u64, isr_75 as *const () as usize as u64,
    isr_76 as *const () as usize as u64, isr_77 as *const () as usize as u64, isr_78 as *const () as usize as u64, isr_79 as *const () as usize as u64,
    isr_80 as *const () as usize as u64, isr_81 as *const () as usize as u64, isr_82 as *const () as usize as u64, isr_83 as *const () as usize as u64,
    isr_84 as *const () as usize as u64, isr_85 as *const () as usize as u64, isr_86 as *const () as usize as u64, isr_87 as *const () as usize as u64,
    isr_88 as *const () as usize as u64, isr_89 as *const () as usize as u64, isr_90 as *const () as usize as u64, isr_91 as *const () as usize as u64,
    isr_92 as *const () as usize as u64, isr_93 as *const () as usize as u64, isr_94 as *const () as usize as u64, isr_95 as *const () as usize as u64,
    isr_96 as *const () as usize as u64, isr_97 as *const () as usize as u64, isr_98 as *const () as usize as u64, isr_99 as *const () as usize as u64,
    isr_100 as *const () as usize as u64, isr_101 as *const () as usize as u64, isr_102 as *const () as usize as u64, isr_103 as *const () as usize as u64,
    isr_104 as *const () as usize as u64, isr_105 as *const () as usize as u64, isr_106 as *const () as usize as u64, isr_107 as *const () as usize as u64,
    isr_108 as *const () as usize as u64, isr_109 as *const () as usize as u64, isr_110 as *const () as usize as u64, isr_111 as *const () as usize as u64,
    isr_112 as *const () as usize as u64, isr_113 as *const () as usize as u64, isr_114 as *const () as usize as u64, isr_115 as *const () as usize as u64,
    isr_116 as *const () as usize as u64, isr_117 as *const () as usize as u64, isr_118 as *const () as usize as u64, isr_119 as *const () as usize as u64,
    isr_120 as *const () as usize as u64, isr_121 as *const () as usize as u64, isr_122 as *const () as usize as u64, isr_123 as *const () as usize as u64,
    isr_124 as *const () as usize as u64, isr_125 as *const () as usize as u64, isr_126 as *const () as usize as u64, isr_127 as *const () as usize as u64,
    isr_128 as *const () as usize as u64, isr_129 as *const () as usize as u64, isr_130 as *const () as usize as u64, isr_131 as *const () as usize as u64,
    isr_132 as *const () as usize as u64, isr_133 as *const () as usize as u64, isr_134 as *const () as usize as u64, isr_135 as *const () as usize as u64,
    isr_136 as *const () as usize as u64, isr_137 as *const () as usize as u64, isr_138 as *const () as usize as u64, isr_139 as *const () as usize as u64,
    isr_140 as *const () as usize as u64, isr_141 as *const () as usize as u64, isr_142 as *const () as usize as u64, isr_143 as *const () as usize as u64,
    isr_144 as *const () as usize as u64, isr_145 as *const () as usize as u64, isr_146 as *const () as usize as u64, isr_147 as *const () as usize as u64,
    isr_148 as *const () as usize as u64, isr_149 as *const () as usize as u64, isr_150 as *const () as usize as u64, isr_151 as *const () as usize as u64,
    isr_152 as *const () as usize as u64, isr_153 as *const () as usize as u64, isr_154 as *const () as usize as u64, isr_155 as *const () as usize as u64,
    isr_156 as *const () as usize as u64, isr_157 as *const () as usize as u64, isr_158 as *const () as usize as u64, isr_159 as *const () as usize as u64,
    isr_160 as *const () as usize as u64, isr_161 as *const () as usize as u64, isr_162 as *const () as usize as u64, isr_163 as *const () as usize as u64,
    isr_164 as *const () as usize as u64, isr_165 as *const () as usize as u64, isr_166 as *const () as usize as u64, isr_167 as *const () as usize as u64,
    isr_168 as *const () as usize as u64, isr_169 as *const () as usize as u64, isr_170 as *const () as usize as u64, isr_171 as *const () as usize as u64,
    isr_172 as *const () as usize as u64, isr_173 as *const () as usize as u64, isr_174 as *const () as usize as u64, isr_175 as *const () as usize as u64,
    isr_176 as *const () as usize as u64, isr_177 as *const () as usize as u64, isr_178 as *const () as usize as u64, isr_179 as *const () as usize as u64,
    isr_180 as *const () as usize as u64, isr_181 as *const () as usize as u64, isr_182 as *const () as usize as u64, isr_183 as *const () as usize as u64,
    isr_184 as *const () as usize as u64, isr_185 as *const () as usize as u64, isr_186 as *const () as usize as u64, isr_187 as *const () as usize as u64,
    isr_188 as *const () as usize as u64, isr_189 as *const () as usize as u64, isr_190 as *const () as usize as u64, isr_191 as *const () as usize as u64,
    isr_192 as *const () as usize as u64, isr_193 as *const () as usize as u64, isr_194 as *const () as usize as u64, isr_195 as *const () as usize as u64,
    isr_196 as *const () as usize as u64, isr_197 as *const () as usize as u64, isr_198 as *const () as usize as u64, isr_199 as *const () as usize as u64,
    isr_200 as *const () as usize as u64, isr_201 as *const () as usize as u64, isr_202 as *const () as usize as u64, isr_203 as *const () as usize as u64,
    isr_204 as *const () as usize as u64, isr_205 as *const () as usize as u64, isr_206 as *const () as usize as u64, isr_207 as *const () as usize as u64,
    isr_208 as *const () as usize as u64, isr_209 as *const () as usize as u64, isr_210 as *const () as usize as u64, isr_211 as *const () as usize as u64,
    isr_212 as *const () as usize as u64, isr_213 as *const () as usize as u64, isr_214 as *const () as usize as u64, isr_215 as *const () as usize as u64,
    isr_216 as *const () as usize as u64, isr_217 as *const () as usize as u64, isr_218 as *const () as usize as u64, isr_219 as *const () as usize as u64,
    isr_220 as *const () as usize as u64, isr_221 as *const () as usize as u64, isr_222 as *const () as usize as u64, isr_223 as *const () as usize as u64,
    isr_224 as *const () as usize as u64, isr_225 as *const () as usize as u64, isr_226 as *const () as usize as u64, isr_227 as *const () as usize as u64,
    isr_228 as *const () as usize as u64, isr_229 as *const () as usize as u64, isr_230 as *const () as usize as u64, isr_231 as *const () as usize as u64,
    isr_232 as *const () as usize as u64, isr_233 as *const () as usize as u64, isr_234 as *const () as usize as u64, isr_235 as *const () as usize as u64,
    isr_236 as *const () as usize as u64, isr_237 as *const () as usize as u64, isr_238 as *const () as usize as u64, isr_239 as *const () as usize as u64,
    isr_240 as *const () as usize as u64, isr_241 as *const () as usize as u64, isr_242 as *const () as usize as u64, isr_243 as *const () as usize as u64,
    isr_244 as *const () as usize as u64, isr_245 as *const () as usize as u64, isr_246 as *const () as usize as u64, isr_247 as *const () as usize as u64,
    isr_248 as *const () as usize as u64, isr_249 as *const () as usize as u64, isr_250 as *const () as usize as u64, isr_251 as *const () as usize as u64,
    isr_252 as *const () as usize as u64, isr_253 as *const () as usize as u64, isr_254 as *const () as usize as u64, isr_255 as *const () as usize as u64,
    ]
}

/// Human-readable names for the 32 architecturally-defined CPU exceptions
/// (Intel SDM Vol 3, Table 6-1), used only for the fault screen's message.
const EXCEPTION_NAMES: [&str; 32] = [
    "Divide Error",
    "Debug",
    "NMI Interrupt",
    "Breakpoint",
    "Overflow",
    "BOUND Range Exceeded",
    "Invalid Opcode",
    "Device Not Available",
    "Double Fault",
    "Coprocessor Segment Overrun (reserved)",
    "Invalid TSS",
    "Segment Not Present",
    "Stack-Segment Fault",
    "General Protection Fault",
    "Page Fault",
    "Reserved",
    "x87 Floating-Point Error",
    "Alignment Check",
    "Machine Check",
    "SIMD Floating-Point Exception",
    "Virtualization Exception",
    "Control Protection Exception",
    "Reserved",
    "Reserved",
    "Reserved",
    "Reserved",
    "Reserved",
    "Reserved",
    "Hypervisor Injection Exception",
    "VMM Communication Exception",
    "Security Exception",
    "Reserved",
];

/// Recover the *true* pre-fault `(rsp, ss)` for the fault screen, correcting
/// for the fact that [`InterruptFrame`]'s `rsp`/`ss` fields are only
/// genuinely hardware-pushed data when the CPU actually switched stacks to
/// take the fault (see the doc comment at this function's one call site for
/// the full reasoning). Only vector 8 (#DF, IST1) switches stacks on this
/// kernel; every other vector's frame has no real RSP/SS to read, so this
/// derives the value instead: for a same-privilege, no-stack-switch fault,
/// RSP at the moment of the fault is exactly the address the CPU's own
/// three-field push (RFLAGS, then CS, then RIP, in push order -- so RFLAGS
/// sits lowest) left behind, i.e. one qword past where RFLAGS was pushed.
/// SS isn't pushed at all in that case, but it's not unknown either: this
/// kernel loads [`gdt::KERNEL_DATA_SELECTOR`] into SS exactly once, in
/// [`gdt::load`], and never changes it afterward (no user mode, no
/// per-task segments), so it's simply reported directly.
fn true_rsp_ss(frame: &InterruptFrame) -> (u64, u64) {
    if frame.vector == 8 {
        (frame.rsp, frame.ss)
    } else {
        let rflags_addr = core::ptr::addr_of!(frame.rflags) as u64;
        (rflags_addr + 8, gdt::KERNEL_DATA_SELECTOR as u64)
    }
}

/// Common handler for every CPU exception (vectors 0-31). Never returns:
/// this kernel has no recovery strategy for a CPU exception (no user mode
/// to kill instead of the kernel, no structured exception handling), so
/// the only safe thing to do is report everything we can onto the fault
/// screen and halt, rather than attempt to resume into unknown state.
extern "C" fn fault_handler_entry(frame: *const InterruptFrame) -> ! {
    // SAFETY: `frame` was just computed as `rsp` by the calling stub,
    // pointing at a fully-populated `InterruptFrame` on the stack of the
    // context that faulted; it is valid for reads for the lifetime of this
    // (never-returning) function call.
    let frame = unsafe { &*frame };
    let vector = frame.vector as usize;
    let name = EXCEPTION_NAMES.get(vector).copied().unwrap_or("Unknown Exception");

    // CR2 holds the faulting linear address only for #PF (14); reading it
    // for any other vector would be meaningless (and, per the SDM, its
    // value after a non-#PF exception is unspecified), so it's read only
    // when relevant and otherwise reported as not applicable.
    let cr2: u64 = if vector == 14 {
        let value: u64;
        unsafe { asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags)) };
        value
    } else {
        0
    };

    let error_code = frame.error_code;
    let rip = frame.rip;
    let cs = frame.cs;
    let rflags = frame.rflags;
    // `frame.rsp`/`frame.ss` are only genuinely hardware-pushed values for
    // vector 8 (#DF): its IST1 entry unconditionally switches stacks (SDM
    // Vol 3, 6.14.5), and a stack switch is exactly what makes the CPU
    // push RSP/SS in the first place. Every other vector here runs with
    // IST=0 and, since this kernel has no user mode yet (CPL is always 0,
    // so a same-privilege-level fault never triggers a stack switch
    // either), the CPU pushes only RIP/CS/RFLAGS -- three fields, not
    // five. Reading `frame.rsp`/`frame.ss` directly in that case doesn't
    // fail or crash (the fields simply overlay whatever pre-existing stack
    // contents sit at those offsets), but it silently reports garbage on
    // the one screen a developer relies on to debug a live fault -- e.g.
    // reporting an `ss` selector value that was never actually loaded into
    // SS, coincidentally landing on stack bytes that can look plausible at
    // a glance but aren't derived from anything real. `true_rsp_ss` below
    // reports the actual values in both cases.
    let (rsp, ss) = true_rsp_ss(frame);
    let (rax, rbx, rcx, rdx) = (frame.rax, frame.rbx, frame.rcx, frame.rdx);
    let (rsi, rdi, rbp) = (frame.rsi, frame.rdi, frame.rbp);
    let (r8, r9, r10, r11) = (frame.r8, frame.r9, frame.r10, frame.r11);
    let (r12, r13, r14, r15) = (frame.r12, frame.r13, frame.r14, frame.r15);

    // For #PF, spell out what the error code means (Intel SDM Vol 3,
    // 4.7): since W^X landed, "which kind of access hit which kind of
    // page" is usually the whole diagnosis -- a write to code/constants,
    // a jump into data, or a touch of an unmapped (guard) page.
    // Built from `&'static str` pieces only -- no allocation on the fault
    // path, which may be reporting a fault *in* the allocator.
    let (pf_label, pf_access, pf_sep, pf_presence, pf_reserved, pf_end) = if vector == 14 {
        let presence = if error_code & 1 != 0 { "protection violation" } else { "page not present" };
        let access = if error_code & (1 << 4) != 0 {
            "instruction fetch"
        } else if error_code & (1 << 1) != 0 {
            "write"
        } else {
            "read"
        };
        let reserved = if error_code & (1 << 3) != 0 { ", reserved bit set" } else { "" };
        ("page fault: ", access, ", ", presence, reserved, "\n")
    } else {
        ("", "", "", "", "", "")
    };

    panic::fault_screen(
        "CPU EXCEPTION",
        format_args!(
            "#{vector} {name}\n\
             {pf_label}{pf_access}{pf_sep}{pf_presence}{pf_reserved}{pf_end}\
             error_code=0x{error_code:x}  cr2=0x{cr2:x}\n\
             rip=0x{rip:x} cs=0x{cs:x}\n\
             rflags=0x{rflags:x}\n\
             rsp=0x{rsp:x} ss=0x{ss:x}\n\
             rax=0x{rax:x} rbx=0x{rbx:x} rcx=0x{rcx:x} rdx=0x{rdx:x}\n\
             rsi=0x{rsi:x} rdi=0x{rdi:x} rbp=0x{rbp:x}\n\
             r8=0x{r8:x} r9=0x{r9:x} r10=0x{r10:x} r11=0x{r11:x}\n\
             r12=0x{r12:x} r13=0x{r13:x} r14=0x{r14:x} r15=0x{r15:x}"
        ),
    )
}

/// Acknowledge a serviced hardware interrupt at whichever controller is
/// actually live right now.
///
/// This kernel has two interrupt controllers over its lifetime -- the
/// legacy 8259 PIC it boots on (`pic.rs`) and the Local APIC it switches
/// to once `ioapic.rs` has routed every line that must keep working
/// (`lapic::enter_apic_mode`) -- and exactly one of them owns any given
/// interrupt. Sending the EOI to the wrong one is not a loud failure: the
/// controller that really had the interrupt in service simply never
/// retires it, and every equal-or-lower-priority interrupt after that is
/// blocked forever, which presents as "the keyboard stopped working"
/// with nothing in any log. Routing every acknowledgement through this
/// one function, keyed off `lapic::is_active()`, is what makes that
/// mistake structurally impossible rather than a thing each call site
/// has to remember.
///
/// `pic_line` is the 8259 IRQ line this vector corresponds to, used only
/// on the pre-switch path.
fn end_of_interrupt(pic_line: u8) {
    if lapic::is_active() {
        lapic::eoi();
    } else {
        pic::end_of_interrupt(pic_line);
    }
}

/// Common handler for every interrupt vector 32-255. Vectors 0x20 (timer
/// -- the PIT before `lapic::start_timer` takes over, the Local APIC
/// timer after) and 0x21 (keyboard) are the only ones this kernel ever
/// routes; every other arm here exists purely to make an unexpected
/// interrupt *visible* (a debugcon log line) instead of silently
/// mishandling it, since reaching them at all means either a hardware
/// glitch (spurious IRQ7/IRQ15 or an APIC spurious vector, all handled
/// correctly below) or a bug in this kernel's own controller setup.
extern "C" fn irq_handler_entry(frame: *const InterruptFrame) {
    // SAFETY: same reasoning as `fault_handler_entry` -- `frame` points at
    // a valid, fully-populated `InterruptFrame` for the duration of this
    // call, and unlike the fault path this function does return, handing
    // control back to the calling stub's restore/`iretq` sequence.
    let vector = unsafe { (*frame).vector };

    // The Local APIC's spurious vector is checked before everything else
    // because it must NOT be acknowledged: the LAPIC never actually
    // accepted an interrupt in this case (it raised the spurious vector
    // precisely because the request went away before it could), so an
    // EOI here would retire whatever genuine interrupt *is* currently in
    // service instead -- the same trap as the 8259's spurious IRQ7,
    // one level up. Intel SDM Vol 3, 11.9.
    if vector == u64::from(lapic::SPURIOUS_VECTOR) {
        debugcon::write_str("\n[idt] spurious LAPIC interrupt (not acknowledged, by design)\n");
        return;
    }

    match vector {
        0x20 => {
            crate::clock::on_tick();
            end_of_interrupt(0);
        }
        0x21 => {
            crate::keyboard::on_irq();
            end_of_interrupt(1);
        }
        0x27 if !lapic::is_active() => {
            // IRQ7 (master) can fire spuriously on real 8259 hardware due
            // to an electrical race between the PIC and CPU; the
            // documented discriminator is that a spurious one never sets
            // the corresponding In-Service Register bit. EOI-ing a
            // spurious IRQ7 would incorrectly tell the PIC a real
            // interrupt was serviced, so it's skipped entirely in that
            // case (no EOI needed or wanted for a spurious master IRQ).
            // Guarded on `!lapic::is_active()`: once the 8259 is fully
            // masked, its In-Service Register is no longer meaningful
            // state to consult, and vector 0x27 can only be a real APIC
            // delivery, handled by the catch-all below.
            if pic::master_isr() & 0x80 != 0 {
                pic::end_of_interrupt(7);
            }
        }
        0x2F if !lapic::is_active() => {
            // IRQ15 (slave)'s spurious case is subtler: even when the
            // slave's own interrupt was spurious, the master PIC's
            // cascade line (IRQ2) *did* see a real edge and still needs
            // its EOI, or the master is left wedged on that priority
            // level. Only the slave's EOI is skipped when spurious.
            if pic::slave_isr() & 0x80 != 0 {
                pic::end_of_interrupt(15);
            } else {
                pic::end_of_interrupt(8); // any line >=8 EOIs the master too; see `pic::end_of_interrupt`
            }
        }
        0x20..=0x2F => {
            // Every other legacy line is masked (in `pic::init` before
            // the switch, and outright in `pic::mask_all` after it) and
            // should never reach here; acknowledge defensively -- so a
            // genuine, unexpected edge doesn't wedge whichever
            // controller is live -- and log so the anomaly is visible
            // rather than silently swallowed.
            let line = (vector - 0x20) as u8;
            debugcon::write_str("\n[idt] unexpected IRQ line ");
            let _ = debugcon::write_fmt(format_args!("{line}\n"));
            end_of_interrupt(line);
        }
        _ => {
            // Vector >= 0x30: never a legacy PIC line. Before the APIC
            // switch there is nothing that could have delivered it and
            // nothing to acknowledge; after it, the Local APIC *did*
            // accept it and now has that priority level in service, so
            // it must be EOI'd or every lower-priority interrupt --
            // including the keyboard -- is blocked from then on. Either
            // way it is a real anomaly worth recording.
            debugcon::write_str("\n[idt] unexpected interrupt vector ");
            let _ = debugcon::write_fmt(format_args!("{vector}\n"));
            if lapic::is_active() {
                lapic::eoi();
            }
        }
    }
}

/// Build the IDT contents (every one of the 256 gates). Pure memory
/// writes, safe to call at any point, including while Boot Services is
/// still active. Must be called before [`load`].
///
/// # Safety
/// Must be called exactly once, before [`load`]; see [`gdt::init`] for the
/// identical reasoning (single-threaded, one-time `static mut`
/// initialization, no CPU descriptor-table state touched here).
pub unsafe fn init() {
    // Computed at runtime, not as a `const`: casting a function pointer to
    // an integer is only defined for runtime evaluation (Rust's const
    // evaluator categorically forbids pointer-to-integer casts, since a
    // function's eventual load address isn't known until link/load time).
    let addrs = stub_addrs();
    unsafe {
        for vector in 0..256usize {
            // Only the double-fault gate uses the dedicated IST1 stack;
            // every other vector runs on whatever stack was already
            // active (IST=0 means "don't switch stacks" -- SDM Vol 3,
            // 6.14.5), which is correct for a kernel with exactly one
            // ring-0 stack and no user mode yet.
            let ist = if vector == 8 { gdt::DOUBLE_FAULT_IST_INDEX as u8 } else { 0 };
            IDT.0[vector] = IdtEntry::new(addrs[vector], ist);
        }
    }
}

/// Load the IDT into the CPU (`lidt`). Per the boot plan, this happens
/// strictly after `exit_boot_services` returns and after [`gdt::load`] (an
/// interrupt gate's selector field names [`gdt::KERNEL_CODE_SELECTOR`],
/// which must already be valid in a loaded GDT before any interrupt --
/// including one that fires the instant after `lidt` executes -- can be
/// taken).
///
/// # Safety
/// Must be called after [`init`] and after [`gdt::load`], and interrupts
/// must still be disabled (the caller enables them with `sti` only after
/// this, `pic::init`, and `pit::init` have all run).
pub unsafe fn load() {
    unsafe {
        let ptr = DescriptorTablePointer {
            limit: (size_of::<Idt>() - 1) as u16,
            base: core::ptr::addr_of!(IDT) as u64,
        };
        asm!("lidt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));
    }
}
