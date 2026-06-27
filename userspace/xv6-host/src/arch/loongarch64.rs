pub(crate) const USER_CONTEXT_WORDS: usize = 32;
pub(crate) const FAULT_REPLY_WORDS: usize = 11;
pub(crate) type FaultReplyFrame = [u64; FAULT_REPLY_WORDS];

// Matches the vendored libsel4 LoongArch64 `seL4_UserContext` order.
const USER_CONTEXT_PC: usize = 0;
const USER_CONTEXT_RA: usize = 1;
const USER_CONTEXT_SP: usize = 2;
const USER_CONTEXT_A0: usize = 16;
const USER_CONTEXT_A1: usize = 17;

const UNKNOWN_SYSCALL_PC: usize = 0;
const UNKNOWN_SYSCALL_SP: usize = 1;
const UNKNOWN_SYSCALL_RA: usize = 2;
const UNKNOWN_SYSCALL_A0: usize = 3;
const UNKNOWN_SYSCALL_A1: usize = 4;
const UNKNOWN_SYSCALL_A2: usize = 5;
const UNKNOWN_SYSCALL_NUMBER: usize = 10;

const VM_FAULT_ADDR: usize = 1;
const VM_FAULT_STATUS: usize = 3;

const SYSCALL_INSTRUCTION_BYTES: u64 = 4;

pub(crate) fn new_user_context(
    entry: u64,
    stack_pointer: u64,
    arg0: u64,
    arg1: u64,
) -> [u64; USER_CONTEXT_WORDS] {
    let mut ctx = [0u64; USER_CONTEXT_WORDS];
    ctx[USER_CONTEXT_PC] = entry;
    ctx[USER_CONTEXT_RA] = 0;
    ctx[USER_CONTEXT_SP] = stack_pointer;
    ctx[USER_CONTEXT_A0] = arg0;
    ctx[USER_CONTEXT_A1] = arg1;
    ctx
}

pub(crate) fn set_user_context_pc(ctx: &mut [u64; USER_CONTEXT_WORDS], pc: u64) {
    ctx[USER_CONTEXT_PC] = pc;
}

pub(crate) fn set_user_context_return_value(ctx: &mut [u64; USER_CONTEXT_WORDS], value: u64) {
    ctx[USER_CONTEXT_A0] = value;
}

pub(crate) fn syscall_number(mrs: &[u64; 64]) -> u64 {
    mrs[UNKNOWN_SYSCALL_NUMBER]
}

pub(crate) fn syscall_arg(mrs: &[u64; 64], index: usize) -> u64 {
    match index {
        0 => mrs[UNKNOWN_SYSCALL_A0],
        1 => mrs[UNKNOWN_SYSCALL_A1],
        2 => mrs[UNKNOWN_SYSCALL_A2],
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
    mrs[UNKNOWN_SYSCALL_PC].wrapping_add(SYSCALL_INSTRUCTION_BYTES)
}

pub(crate) fn syscall_reply_frame(mrs: &[u64; 64]) -> FaultReplyFrame {
    let mut reply = [0u64; FAULT_REPLY_WORDS];
    reply.copy_from_slice(&mrs[..FAULT_REPLY_WORDS]);
    reply[UNKNOWN_SYSCALL_PC] = resumed_fault_pc(mrs);
    reply
}

pub(crate) fn set_syscall_return_value(reply: &mut FaultReplyFrame, value: u64) {
    reply[UNKNOWN_SYSCALL_A0] = value;
}

pub(crate) fn exec_reply_frame(
    entry: u64,
    stack_pointer: u64,
    arg0: u64,
    arg1: u64,
) -> FaultReplyFrame {
    let mut reply = [0u64; FAULT_REPLY_WORDS];
    reply[UNKNOWN_SYSCALL_PC] = entry;
    reply[UNKNOWN_SYSCALL_SP] = stack_pointer;
    reply[UNKNOWN_SYSCALL_RA] = 0;
    reply[UNKNOWN_SYSCALL_A0] = arg0;
    reply[UNKNOWN_SYSCALL_A1] = arg1;
    reply
}
