/// libsel4 `seL4_UserContext` on x86_64 (rip…r15, fs_base, gs_base).
/// Wider than this fails `TCB_ReadRegisters` (`count > 24`).
pub(crate) const USER_CONTEXT_WORDS: usize = 20;
pub(crate) const FAULT_REPLY_WORDS: usize = 19;
pub(crate) type FaultReplyFrame = [u64; FAULT_REPLY_WORDS];

pub(crate) const EXPECTED_ELF_MACHINE: u16 = 62;
pub(crate) const UTS_MACHINE: &[u8] = b"x86_64";

const USER_CONTEXT_PC: usize = 0;
const USER_CONTEXT_SP: usize = 1;
const USER_CONTEXT_RFLAGS: usize = 2;
const USER_CONTEXT_A1: usize = 7;
const USER_CONTEXT_A0: usize = 8;
const USER_CONTEXT_RSI: usize = USER_CONTEXT_A1;
const USER_CONTEXT_RDI: usize = USER_CONTEXT_A0;

const UNKNOWN_SYSCALL_RAX: usize = 0;
const UNKNOWN_SYSCALL_RDX: usize = 3;
const UNKNOWN_SYSCALL_RSI: usize = 4;
const UNKNOWN_SYSCALL_RDI: usize = 5;
const UNKNOWN_SYSCALL_R8: usize = 7;
const UNKNOWN_SYSCALL_R9: usize = 8;
const UNKNOWN_SYSCALL_R10: usize = 9;
const UNKNOWN_SYSCALL_FAULT_IP: usize = 15;
const UNKNOWN_SYSCALL_SP: usize = 16;

const VM_FAULT_ADDR: usize = 1;
const VM_FAULT_STATUS: usize = 3;

const USER_RFLAGS: u64 = 0x202;

pub(crate) fn new_user_context(
    entry: u64,
    stack_pointer: u64,
    arg0: u64,
    arg1: u64,
) -> [u64; USER_CONTEXT_WORDS] {
    let mut ctx = [0u64; USER_CONTEXT_WORDS];
    ctx[USER_CONTEXT_PC] = entry;
    ctx[USER_CONTEXT_SP] = stack_pointer;
    ctx[USER_CONTEXT_RFLAGS] = USER_RFLAGS;
    ctx[USER_CONTEXT_RDI] = arg0;
    ctx[USER_CONTEXT_RSI] = arg1;
    ctx
}

pub(crate) fn set_user_context_pc(ctx: &mut [u64; USER_CONTEXT_WORDS], pc: u64) {
    ctx[USER_CONTEXT_PC] = pc;
}

pub(crate) fn set_user_context_return_value(ctx: &mut [u64; USER_CONTEXT_WORDS], value: u64) {
    ctx[3] = value;
}

pub(crate) fn syscall_number(mrs: &[u64; 64]) -> u64 {
    mrs[UNKNOWN_SYSCALL_RAX]
}

pub(crate) fn syscall_arg(mrs: &[u64; 64], index: usize) -> u64 {
    match index {
        0 => mrs[UNKNOWN_SYSCALL_RDI],
        1 => mrs[UNKNOWN_SYSCALL_RSI],
        2 => mrs[UNKNOWN_SYSCALL_RDX],
        3 => mrs[UNKNOWN_SYSCALL_R10],
        4 => mrs[UNKNOWN_SYSCALL_R8],
        5 => mrs[UNKNOWN_SYSCALL_R9],
        _ => 0,
    }
}

pub(crate) fn vm_fault_addr(mrs: &[u64; 64]) -> u64 {
    mrs[VM_FAULT_ADDR]
}

pub(crate) fn vm_fault_status(mrs: &[u64; 64]) -> u64 {
    mrs[VM_FAULT_STATUS]
}

pub(crate) fn resumed_fault_pc(mrs: &[u64; 64]) -> u64 {
    mrs[UNKNOWN_SYSCALL_FAULT_IP]
}

pub(crate) fn syscall_reply_frame(mrs: &[u64; 64]) -> FaultReplyFrame {
    let mut reply = [0u64; FAULT_REPLY_WORDS];
    reply.copy_from_slice(&mrs[..FAULT_REPLY_WORDS]);
    reply[UNKNOWN_SYSCALL_FAULT_IP] = resumed_fault_pc(mrs);
    reply
}

pub(crate) fn set_syscall_return_value(reply: &mut FaultReplyFrame, value: u64) {
    reply[UNKNOWN_SYSCALL_RAX] = value;
}

pub(crate) fn exec_reply_frame(entry: u64, stack_pointer: u64) -> FaultReplyFrame {
    let mut reply = [0u64; FAULT_REPLY_WORDS];
    reply[UNKNOWN_SYSCALL_FAULT_IP] = entry;
    reply[UNKNOWN_SYSCALL_SP] = stack_pointer;
    reply
}
