use crate::allocator::Allocator;
use crate::arch::current as arch;
use crate::child::{
    copy_cstr_from_child, copy_from_child, copy_to_child, load_elf, map_stack,
    reset_process_mappings, write_user_context,
};
use crate::consts::*;
use crate::types::{SyscallResult, TaskStruct};
use crate::util::{LogBytes, info, read_u64, write_u64_bytes};
use crate::vfs::{basename, vfs_read_exec_image};

pub(crate) fn load_init_program(alloc: &mut Allocator, child: &mut TaskStruct, path: &[u8]) {
    let Some(image) = vfs_read_exec_image(child, path) else {
        crate::util::warn!("linux-compat: missing init {}", LogBytes(path));
        crate::util::halt_loop();
    };
    load_elf(alloc, child, image);
    map_stack(alloc, child);
    let mut args = [[0u8; MAX_EXEC_ARG_LEN]; MAX_EXEC_ARGS];
    let mut arg_lens = [0usize; MAX_EXEC_ARGS];
    let n = core::cmp::min(path.len(), MAX_EXEC_ARG_LEN);
    args[0][..n].copy_from_slice(&path[..n]);
    arg_lens[0] = n;
    set_exec_path(child, path);
    let empty_envs = [[0u8; MAX_EXEC_ARG_LEN]; MAX_EXEC_ARGS];
    let empty_env_lens = [0usize; MAX_EXEC_ARGS];
    let Some(sp) = setup_linux_stack(
        alloc,
        child,
        &args,
        &arg_lens,
        1,
        &empty_envs,
        &empty_env_lens,
        0,
    ) else {
        crate::util::warn!("linux-compat: failed to build init stack");
        crate::util::halt_loop();
    };
    let ctx = arch::new_user_context(child.entry, sp, 0, 0);
    write_user_context(child.tcb, &ctx, true);
    info!("linux-compat: exec {} pid={}", LogBytes(path), child.pid);
}

pub(crate) fn sys_execve(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    path_ptr: u64,
    argv_ptr: u64,
    envp_ptr: u64,
) -> SyscallResult {
    let mut path = [0u8; 128];
    let Some(path_len) = copy_cstr_from_child(alloc, child, path_ptr, &mut path) else {
        return SyscallResult::err(EFAULT);
    };
    let mut args = [[0u8; MAX_EXEC_ARG_LEN]; MAX_EXEC_ARGS];
    let mut arg_lens = [0usize; MAX_EXEC_ARGS];
    let Some(argc) = collect_exec_args(alloc, child, argv_ptr, &mut args, &mut arg_lens) else {
        return SyscallResult::err(E2BIG);
    };
    let mut envs = [[0u8; MAX_EXEC_ARG_LEN]; MAX_EXEC_ARGS];
    let mut env_lens = [0usize; MAX_EXEC_ARGS];
    let envc = if envp_ptr == 0 {
        0
    } else {
        collect_exec_args(alloc, child, envp_ptr, &mut envs, &mut env_lens).unwrap_or(0)
    };

    let path_bytes = &path[..path_len];
    let Some(image) = vfs_read_exec_image(child, path_bytes) else {
        return SyscallResult::err(ENOENT);
    };
    let name = basename(path_bytes);

    reset_process_mappings(alloc, child.pid);
    load_elf(alloc, child, image);
    map_stack(alloc, child);
    set_exec_path(child, path_bytes);
    let Some(sp) = setup_linux_stack(alloc, child, &args, &arg_lens, argc, &envs, &env_lens, envc)
    else {
        return SyscallResult::err(ENOMEM);
    };

    let ctx = arch::new_user_context(child.entry, sp, 0, 0);
    write_user_context(child.tcb, &ctx, false);
    let reply = arch::exec_reply_frame(child.entry, sp);
    info!("linux-compat: exec {} pid={}", LogBytes(name), child.pid);
    SyscallResult::ReplyFrame(reply)
}

fn set_exec_path(child: &mut TaskStruct, path: &[u8]) {
    child.exec_path = [0; MAX_PATH_BYTES];
    let n = core::cmp::min(path.len(), MAX_PATH_BYTES);
    child.exec_path[..n].copy_from_slice(&path[..n]);
    child.exec_path_len = n;
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

fn setup_linux_stack(
    alloc: &mut Allocator,
    child: &TaskStruct,
    args: &[[u8; MAX_EXEC_ARG_LEN]; MAX_EXEC_ARGS],
    arg_lens: &[usize; MAX_EXEC_ARGS],
    argc: usize,
    envs: &[[u8; MAX_EXEC_ARG_LEN]; MAX_EXEC_ARGS],
    env_lens: &[usize; MAX_EXEC_ARGS],
    envc: usize,
) -> Option<u64> {
    let mut sp = child.heap_start;
    let stack_base = child
        .heap_start
        .checked_sub(PAGE_SIZE * CHILD_STACK_PAGES as u64)?;
    let mut arg_ptrs = [0u64; MAX_EXEC_ARGS];
    let mut env_ptrs = [0u64; MAX_EXEC_ARGS];

    for i in 0..argc {
        let len = arg_lens[i];
        sp = push_cstr(alloc, child, sp, stack_base, &args[i][..len])?;
        arg_ptrs[i] = sp;
    }
    for i in 0..envc {
        let len = env_lens[i];
        sp = push_cstr(alloc, child, sp, stack_base, &envs[i][..len])?;
        env_ptrs[i] = sp;
    }
    let execfn = if child.exec_path_len > 0 {
        sp = push_cstr(
            alloc,
            child,
            sp,
            stack_base,
            &child.exec_path[..child.exec_path_len],
        )?;
        sp
    } else {
        0
    };

    sp = sp.checked_sub(16)?;
    sp &= !0xf;
    if sp < stack_base {
        return None;
    }
    let mut random = [0u8; 16];
    let mut seed = child.pid.wrapping_mul(0x9e37_79b9);
    let mut i = 0usize;
    while i < 16 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        random[i] = (seed >> 24) as u8;
        i += 1;
    }
    if !copy_to_child(alloc, child, sp, &random) {
        return None;
    }
    let random_ptr = sp;

    let aux = [
        AT_PHDR,
        child.elf_phdr,
        AT_PHENT,
        child.elf_phent as u64,
        AT_PHNUM,
        child.elf_phnum as u64,
        AT_PAGESZ,
        PAGE_SIZE,
        AT_BASE,
        0,
        AT_FLAGS,
        0,
        AT_ENTRY,
        child.entry,
        AT_UID,
        0,
        AT_EUID,
        0,
        AT_GID,
        0,
        AT_EGID,
        0,
        AT_SECURE,
        0,
        AT_CLKTCK,
        100,
        AT_RANDOM,
        random_ptr,
        AT_EXECFN,
        execfn,
        AT_NULL,
        0,
    ];

    sp &= !0xf;
    for pair in aux.chunks(2).rev() {
        sp = write_u64_at(alloc, child, sp, stack_base, pair[1])?;
        sp = write_u64_at(alloc, child, sp, stack_base, pair[0])?;
    }
    sp = write_u64_at(alloc, child, sp, stack_base, 0)?;
    for i in (0..envc).rev() {
        sp = write_u64_at(alloc, child, sp, stack_base, env_ptrs[i])?;
    }
    sp = write_u64_at(alloc, child, sp, stack_base, 0)?;
    for i in (0..argc).rev() {
        sp = write_u64_at(alloc, child, sp, stack_base, arg_ptrs[i])?;
    }
    sp = write_u64_at(alloc, child, sp, stack_base, argc as u64)?;
    Some(sp)
}

fn push_cstr(
    alloc: &mut Allocator,
    child: &TaskStruct,
    mut sp: u64,
    stack_base: u64,
    bytes: &[u8],
) -> Option<u64> {
    sp = sp.checked_sub((bytes.len() + 1) as u64)?;
    if sp < stack_base {
        return None;
    }
    if !copy_to_child(alloc, child, sp, bytes) {
        return None;
    }
    if !copy_to_child(alloc, child, sp + bytes.len() as u64, &[0]) {
        return None;
    }
    Some(sp)
}

fn write_u64_at(
    alloc: &mut Allocator,
    child: &TaskStruct,
    mut sp: u64,
    stack_base: u64,
    value: u64,
) -> Option<u64> {
    sp = sp.checked_sub(8)?;
    if sp < stack_base {
        return None;
    }
    if !write_child_u64(alloc, child, sp, value) {
        return None;
    }
    Some(sp)
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

const E2BIG: i32 = 7;
