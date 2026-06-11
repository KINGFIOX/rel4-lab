use crate::allocator::Allocator;
use crate::child::{
    USER_CONTEXT_WORDS, copy_cstr_from_child, copy_from_child, copy_to_child, load_elf, map_stack,
    reset_process_mappings, write_user_context,
};
use crate::consts::{MAX_EXEC_ARG_LEN, MAX_EXEC_ARGS};
use crate::types::{SyscallResult, TaskStruct};
use crate::util::{LogBytes, info, read_u64, write_u64_bytes};
use crate::vfs::{basename, vfs_read_exec_image};

pub(crate) fn sys_exec(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    path_ptr: u64,
    argv_ptr: u64,
) -> SyscallResult {
    let mut path = [0u8; 128];
    let Some(path_len) = copy_cstr_from_child(alloc, child, path_ptr, &mut path) else {
        return SyscallResult::Reply(-1);
    };
    let mut args = [[0u8; MAX_EXEC_ARG_LEN]; MAX_EXEC_ARGS];
    let mut arg_lens = [0usize; MAX_EXEC_ARGS];
    let Some(argc) = collect_exec_args(alloc, child, argv_ptr, &mut args, &mut arg_lens) else {
        return SyscallResult::Reply(-1);
    };

    let path_bytes = &path[..path_len];
    let Some(image) = vfs_read_exec_image(child, path_bytes) else {
        return SyscallResult::Reply(-1);
    };
    let name = basename(path_bytes);

    reset_process_mappings(alloc, child.pid);
    load_elf(alloc, child, image);
    map_stack(alloc, child);
    let Some((sp, argv_va)) = setup_exec_stack(alloc, child, &args, &arg_lens, argc) else {
        return SyscallResult::Reply(-1);
    };

    let mut ctx = [0u64; USER_CONTEXT_WORDS];
    ctx[0] = child.entry;
    ctx[2] = sp;
    ctx[16] = argc as u64;
    ctx[17] = argv_va;
    write_user_context(child.tcb, &ctx, false);

    let mut reply = [0u64; 11];
    reply[0] = child.entry;
    reply[1] = sp;
    reply[2] = 0;
    reply[3] = argc as u64;
    reply[4] = argv_va;
    info!("xv6-host: exec {} pid={}", LogBytes(name), child.pid);
    SyscallResult::ReplyFrame(reply)
}

fn collect_exec_args(
    alloc: &mut Allocator,
    child: &TaskStruct,
    argv_ptr: u64,
    args: &mut [[u8; MAX_EXEC_ARG_LEN]; MAX_EXEC_ARGS],
    arg_lens: &mut [usize; MAX_EXEC_ARGS],
) -> Option<usize> {
    let mut argc = 0;
    loop {
        if argc >= MAX_EXEC_ARGS {
            return None;
        }
        let ptr = read_child_u64(alloc, child, argv_ptr + (argc as u64 * 8))?;
        if ptr == 0 {
            return Some(argc);
        }
        let len = copy_cstr_from_child(alloc, child, ptr, &mut args[argc])?;
        arg_lens[argc] = len;
        argc += 1;
    }
}

fn setup_exec_stack(
    alloc: &mut Allocator,
    child: &TaskStruct,
    args: &[[u8; MAX_EXEC_ARG_LEN]; MAX_EXEC_ARGS],
    arg_lens: &[usize; MAX_EXEC_ARGS],
    argc: usize,
) -> Option<(u64, u64)> {
    let mut sp = child.heap_start;
    let mut arg_ptrs = [0u64; MAX_EXEC_ARGS];
    for i in 0..argc {
        let len = arg_lens[i];
        sp = sp.checked_sub((len + 1) as u64)?;
        if !copy_to_child(alloc, child, sp, &args[i][..len]) {
            return None;
        }
        if !copy_to_child(alloc, child, sp + len as u64, &[0]) {
            return None;
        }
        arg_ptrs[i] = sp;
    }

    sp &= !0xf;
    sp = sp.checked_sub(8)?;
    if !write_child_u64(alloc, child, sp, 0) {
        return None;
    }
    for i in (0..argc).rev() {
        sp = sp.checked_sub(8)?;
        if !write_child_u64(alloc, child, sp, arg_ptrs[i]) {
            return None;
        }
    }
    let argv_va = sp;
    sp &= !0xf;
    Some((sp, argv_va))
}

fn read_child_u64(alloc: &mut Allocator, child: &TaskStruct, va: u64) -> Option<u64> {
    let mut bytes = [0u8; 8];
    if !copy_from_child(alloc, child, va, &mut bytes) {
        return None;
    }
    Some(read_u64(&bytes, 0))
}

fn write_child_u64(alloc: &mut Allocator, child: &TaskStruct, va: u64, value: u64) -> bool {
    let mut bytes = [0u8; 8];
    write_u64_bytes(&mut bytes, 0, value);
    copy_to_child(alloc, child, va, &bytes)
}
