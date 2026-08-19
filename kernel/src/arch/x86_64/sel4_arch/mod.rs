//! x86_64 seL4 ABI surface. Register indices follow upstream
//! `arch/x86/arch/64/mode/machine/registerset.h`.

pub mod invocation;
pub mod object_type;

pub use object_type::ObjectType;

/// Opaque address-space root programmed by `switch_vspace`.
pub type VspaceRoot = u64;

use crate::object::cap::{Cap, CapTag};

pub const SEL4_USER_CONTEXT_WORDS: usize = 24;
/// Visible `seL4_UserContext` fields: 18 frame registers plus FS/GS.
pub const SEL4_USER_CONTEXT_ABI_WORDS: usize = 20;

const RDI: usize = 0;
const RSI: usize = 1;
const RAX: usize = 2;
const RBX: usize = 3;
const RBP: usize = 4;
const R12: usize = 5;
const R13: usize = 6;
const R14: usize = 7;
const RDX: usize = 8;
const R10: usize = 9;
const R8: usize = 10;
const R9: usize = 11;
const R15: usize = 12;
const FLAGS: usize = 13;
const NEXT_IP: usize = 14;
const RSP: usize = 16;
const FAULT_IP: usize = 17;
const R11: usize = 18;
const RCX: usize = 19;
const FS_BASE: usize = 22;

/// User-visible TCB register ABI indices for Read/WriteRegisters.
/// Order matches seL4 `frameRegisters[]` then `gpRegisters[]` /
/// `seL4_UserContext` on x86_64: rip, rsp, rflags, rax, rbx, rcx, rdx,
/// rsi, rdi, rbp, r8–r15, fs_base, gs_base.
pub const SEL4_USER_CONTEXT_REGS: [usize; 32] = [
    UserRegister::Rip.index(),
    UserRegister::Rsp.index(),
    UserRegister::Rflags.index(),
    UserRegister::Rax.index(),
    3,
    19,
    8,
    UserRegister::Rsi.index(),
    UserRegister::Rdi.index(),
    4,
    UserRegister::R8.index(),
    UserRegister::R9.index(),
    UserRegister::R10.index(),
    18,
    5,
    6,
    7,
    UserRegister::R15.index(),
    UserRegister::FsBase.index(),
    23,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
];
pub const SEL4_TCB_FRAME_REGS: [usize; 18] = [
    UserRegister::Rip.index(),
    UserRegister::Rsp.index(),
    UserRegister::Rflags.index(),
    UserRegister::Rax.index(),
    3,
    19,
    8,
    UserRegister::Rsi.index(),
    UserRegister::Rdi.index(),
    4,
    UserRegister::R8.index(),
    UserRegister::R9.index(),
    UserRegister::R10.index(),
    18,
    5,
    6,
    7,
    UserRegister::R15.index(),
];
pub const SEL4_TCB_GP_REGS: [usize; 16] = [
    UserRegister::FsBase.index(),
    23,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
];

#[repr(C, align(16))]
#[derive(Copy, Clone)]
pub struct FpuState {
    pub fxsave: [u8; 512],
}

impl Default for FpuState {
    fn default() -> Self {
        Self::zero()
    }
}

impl FpuState {
    pub const fn zero() -> Self {
        Self { fxsave: [0; 512] }
    }
}

/// seL4 x86_64 user context. `regs` is the kernel trap-save array, not the
/// compact libsel4 `seL4_UserContext` struct.
#[repr(C)]
#[derive(Default)]
pub struct UserContext {
    pub regs: [u64; SEL4_USER_CONTEXT_WORDS],
    pub pc: u64,
    pub restart_pc: u64,
    pub fpu: FpuState,
}

impl UserContext {
    pub const fn zero() -> Self {
        Self {
            regs: [0; SEL4_USER_CONTEXT_WORDS],
            pc: 0,
            restart_pc: 0,
            fpu: FpuState::zero(),
        }
    }

    pub fn cap_reg(&self) -> u64 {
        self.regs[RDI]
    }

    pub fn set_cap_reg(&mut self, value: u64) {
        self.regs[RDI] = value;
    }

    pub fn msg_info(&self) -> u64 {
        self.regs[RSI]
    }

    pub fn set_msg_info(&mut self, value: u64) {
        self.regs[RSI] = value;
    }

    pub fn mr(&self, index: usize) -> u64 {
        match index {
            0 => self.regs[R10],
            1 => self.regs[R8],
            2 => self.regs[R9],
            3 => self.regs[R15],
            _ => 0,
        }
    }

    pub fn set_mr(&mut self, index: usize, value: u64) {
        let slot = match index {
            0 => R10,
            1 => R8,
            2 => R9,
            3 => R15,
            _ => return,
        };
        self.regs[slot] = value;
    }

    pub fn reply_reg(&self) -> u64 {
        self.regs[5]
    }

    pub fn set_reply_reg(&mut self, value: u64) {
        self.regs[5] = value;
    }

    pub fn syscall_reg(&self) -> u64 {
        self.regs[RAX]
    }

    /// seL4 x86_64 libsel4 places the syscall number in RDX.
    pub fn libsel4_syscall_reg(&self) -> u64 {
        self.regs[RDX]
    }

    pub fn stack_reg(&self) -> u64 {
        self.regs[RSP]
    }

    pub fn set_stack_reg(&mut self, value: u64) {
        self.regs[RSP] = value;
    }

    pub fn return_reg(&self) -> u64 {
        self.regs[RAX]
    }

    pub fn set_return_reg(&mut self, value: u64) {
        self.regs[RAX] = value;
    }

    pub fn tls_reg(&self) -> u64 {
        self.regs[FS_BASE]
    }

    pub fn set_tls_reg(&mut self, value: u64) {
        self.regs[FS_BASE] = value;
    }

    pub fn scratch_reg(&self) -> u64 {
        self.regs[RAX]
    }

    pub fn clear_ipc_regs(&mut self) {
        self.set_cap_reg(0);
        self.set_msg_info(0);
        self.set_mr(0, 0);
        self.set_mr(1, 0);
        self.set_mr(2, 0);
        self.set_mr(3, 0);
    }
}

/// seL4 x86_64 syscall-register aliases used by shared TCB/IPC helpers.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum UserRegister {
    Rip,
    Rsp,
    Rflags,
    Rax,
    Rdi,
    Rsi,
    R10,
    R8,
    R9,
    R15,
    FsBase,
}

impl UserRegister {
    pub const fn index(self) -> usize {
        match self {
            Self::Rip => FAULT_IP,
            Self::Rsp => RSP,
            Self::Rflags => FLAGS,
            Self::Rax => RAX,
            Self::Rdi => RDI,
            Self::Rsi => RSI,
            Self::R10 => R10,
            Self::R8 => R8,
            Self::R9 => R9,
            Self::R15 => R15,
            Self::FsBase => FS_BASE,
        }
    }
}

/// seL4 x86_64 UnknownSyscall message: RAX…R15, FaultIP, SP, FLAGS, Syscall.
pub const UNKNOWN_SYSCALL_LENGTH: u64 = 19;
pub const UNKNOWN_SYSCALL_FAULT_IP_MR: usize = 15;
pub const UNKNOWN_SYSCALL_REPLY_REGS: &[usize] = &[
    RAX, RBX, RCX, RDX, RSI, RDI, RBP, R8, R9, R10, R11, R12, R13, R14, R15, FAULT_IP, RSP, FLAGS,
    0,
];
pub const USER_EXCEPTION_SP_REG: usize = UserRegister::Rsp.index();

pub fn init_user_context(context: &mut UserContext) {
    context.regs[FLAGS] = 0x202;
}

pub fn init_rootserver_context(context: &mut UserContext, entry: u64, stack: u64, bootinfo: u64) {
    context.pc = entry;
    context.restart_pc = entry;
    context.regs[NEXT_IP] = entry;
    context.regs[FAULT_IP] = entry;
    context.regs[FLAGS] = 0x202;
    context.set_cap_reg(bootinfo);
    context.set_msg_info(0);
    context.set_stack_reg(stack);
}

pub fn set_fpu_context_enabled(_context: &mut UserContext, _enabled: bool) {}

pub fn apply_written_pc(context: &mut UserContext, pc: u64) {
    context.pc = pc;
    context.restart_pc = pc;
    context.regs[NEXT_IP] = pc;
    context.regs[FAULT_IP] = pc;
}

pub fn apply_preemption_restart(context: &mut UserContext) {
    apply_written_pc(context, context.restart_pc);
}

pub fn same_object_as(left: Cap, right: Cap) -> bool {
    match (left.tag(), right.tag()) {
        (Some(CapTag::Endpoint), Some(CapTag::Endpoint)) => {
            left.endpoint_ptr() == right.endpoint_ptr()
        }
        (Some(CapTag::Notification), Some(CapTag::Notification)) => {
            left.notification_ptr() == right.notification_ptr()
        }
        (Some(CapTag::CNode), Some(CapTag::CNode)) => {
            left.cnode_ptr() == right.cnode_ptr() && left.cnode_radix() == right.cnode_radix()
        }
        (Some(CapTag::Thread), Some(CapTag::Thread)) => left.thread_ptr() == right.thread_ptr(),
        (Some(CapTag::Reply), Some(CapTag::Reply)) => {
            left.reply_object_ptr() == right.reply_object_ptr()
        }
        (Some(CapTag::IrqHandler), Some(CapTag::IrqHandler)) => {
            left.irq_handler_irq() == right.irq_handler_irq()
        }
        (Some(CapTag::IoPort), Some(CapTag::IoPort)) => {
            left.io_port_first() == right.io_port_first()
                && left.io_port_last() == right.io_port_last()
        }
        (Some(CapTag::Domain), Some(CapTag::Domain))
        | (Some(CapTag::AsidControl), Some(CapTag::AsidControl))
        | (Some(CapTag::IoPortControl), Some(CapTag::IoPortControl)) => true,
        (Some(CapTag::PageTable), Some(CapTag::PageTable)) => {
            left.page_table_base_ptr() == right.page_table_base_ptr()
        }
        (Some(CapTag::AsidPool), Some(CapTag::AsidPool)) => {
            left.asid_pool_ptr() == right.asid_pool_ptr()
        }
        (Some(CapTag::Frame), Some(CapTag::Frame)) => {
            left.frame_base_ptr() == right.frame_base_ptr()
                && left.frame_size() == right.frame_size()
                && left.frame_is_device() == right.frame_is_device()
        }
        _ => false,
    }
}
