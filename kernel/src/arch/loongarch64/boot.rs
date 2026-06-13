//! LoongArch64 boot entry placeholder.

compile_error!(
    "LoongArch64 boot backend is not implemented yet; add the seL4 LoongArch64 elfloader entry ABI before building this target"
);

pub fn _start() -> ! {
    loop {}
}

pub fn init_kernel(
    _a0: usize,
    _a1: usize,
    _a2: usize,
    _a3: usize,
    _a4: usize,
    _a5: usize,
    _a6: usize,
    _a7: usize,
) -> ! {
    loop {}
}

pub fn halt() -> ! {
    loop {}
}
