//! RISC-V 64-bit seL4 ABI surface: user context accessors, invocation
//! labels, and object-type IDs. Shared kernel code talks to this module
//! instead of RISC-V register or SATP names.

pub mod invocation;
pub mod object_type;

pub use crate::arch::riscv64::kernel::trap::{
    ROOTSERVER_SSTATUS, SEL4_TCB_FRAME_REGS, SEL4_TCB_GP_REGS, SEL4_USER_CONTEXT_REGS,
    SEL4_USER_CONTEXT_WORDS, SSTATUS_FS_CLEAN, SSTATUS_FS_MASK, USER_SSTATUS, UserContext,
    UserRegister,
};
pub use object_type::ObjectType;

/// Opaque address-space root programmed by `switch_vspace`.
pub type VspaceRoot = u64;

use crate::arch::riscv64::kernel::trap::UserRegister as Reg;

impl UserContext {
    pub fn cap_reg(&self) -> u64 {
        self.regs[Reg::A0.index()]
    }

    pub fn set_cap_reg(&mut self, value: u64) {
        self.regs[Reg::A0.index()] = value;
    }

    pub fn msg_info(&self) -> u64 {
        self.regs[Reg::A1.index()]
    }

    pub fn set_msg_info(&mut self, value: u64) {
        self.regs[Reg::A1.index()] = value;
    }

    pub fn mr(&self, index: usize) -> u64 {
        match index {
            0 => self.regs[Reg::A2.index()],
            1 => self.regs[Reg::A3.index()],
            2 => self.regs[Reg::A4.index()],
            3 => self.regs[Reg::A5.index()],
            _ => 0,
        }
    }

    pub fn set_mr(&mut self, index: usize, value: u64) {
        let slot = match index {
            0 => Reg::A2.index(),
            1 => Reg::A3.index(),
            2 => Reg::A4.index(),
            3 => Reg::A5.index(),
            _ => return,
        };
        self.regs[slot] = value;
    }

    pub fn reply_reg(&self) -> u64 {
        self.regs[Reg::A6.index()]
    }

    pub fn set_reply_reg(&mut self, value: u64) {
        self.regs[Reg::A6.index()] = value;
    }

    pub fn syscall_reg(&self) -> u64 {
        self.regs[Reg::A7.index()]
    }

    pub fn stack_reg(&self) -> u64 {
        self.regs[Reg::Sp.index()]
    }

    pub fn set_stack_reg(&mut self, value: u64) {
        self.regs[Reg::Sp.index()] = value;
    }

    pub fn return_reg(&self) -> u64 {
        self.regs[Reg::Ra.index()]
    }

    pub fn set_return_reg(&mut self, value: u64) {
        self.regs[Reg::Ra.index()] = value;
    }

    pub fn tls_reg(&self) -> u64 {
        self.regs[Reg::Tp.index()]
    }

    pub fn set_tls_reg(&mut self, value: u64) {
        self.regs[Reg::Tp.index()] = value;
    }

    pub fn scratch_reg(&self) -> u64 {
        self.regs[Reg::T0.index()]
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

/// Fault-reply register slots. Index 0 is the fault PC sentinel.
pub const UNKNOWN_SYSCALL_REPLY_REGS: [usize; 10] = [
    0,
    UserRegister::Sp.index(),
    UserRegister::Ra.index(),
    UserRegister::A0.index(),
    UserRegister::A1.index(),
    UserRegister::A2.index(),
    UserRegister::A3.index(),
    UserRegister::A4.index(),
    UserRegister::A5.index(),
    UserRegister::A6.index(),
];
pub const USER_EXCEPTION_SP_REG: usize = UserRegister::Sp.index();

pub fn init_user_context(context: &mut UserContext) {
    context.sstatus = USER_SSTATUS;
}

pub fn init_rootserver_context(context: &mut UserContext, entry: u64, stack: u64, bootinfo: u64) {
    context.pc = entry;
    context.restart_pc = entry;
    context.sstatus = ROOTSERVER_SSTATUS;
    context.set_cap_reg(bootinfo);
    context.set_msg_info(0);
    context.set_stack_reg(stack);
}

pub fn set_fpu_context_enabled(context: &mut UserContext, enabled: bool) {
    let status = context.sstatus & !SSTATUS_FS_MASK;
    context.sstatus = if enabled {
        status | SSTATUS_FS_CLEAN
    } else {
        status
    };
}
