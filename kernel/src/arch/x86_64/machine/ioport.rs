//! Kernel `in` / `out` for issued IoPort caps.

#[inline]
pub fn in8(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!(
            "inb %dx, %al",
            in("dx") port,
            out("al") value,
            options(att_syntax, nomem, nostack, preserves_flags),
        );
    }
    value
}

#[inline]
pub fn in16(port: u16) -> u16 {
    let value: u16;
    unsafe {
        core::arch::asm!(
            "inw %dx, %ax",
            in("dx") port,
            out("ax") value,
            options(att_syntax, nomem, nostack, preserves_flags),
        );
    }
    value
}

#[inline]
pub fn in32(port: u16) -> u32 {
    let value: u32;
    unsafe {
        core::arch::asm!(
            "inl %dx, %eax",
            in("dx") port,
            out("eax") value,
            options(att_syntax, nomem, nostack, preserves_flags),
        );
    }
    value
}

#[inline]
pub fn out8(port: u16, value: u8) {
    unsafe {
        core::arch::asm!(
            "outb %al, %dx",
            in("dx") port,
            in("al") value,
            options(att_syntax, nomem, nostack, preserves_flags),
        );
    }
}

#[inline]
pub fn out16(port: u16, value: u16) {
    unsafe {
        core::arch::asm!(
            "outw %ax, %dx",
            in("dx") port,
            in("ax") value,
            options(att_syntax, nomem, nostack, preserves_flags),
        );
    }
}

#[inline]
pub fn out32(port: u16, value: u32) {
    unsafe {
        core::arch::asm!(
            "outl %eax, %dx",
            in("dx") port,
            in("eax") value,
            options(att_syntax, nomem, nostack, preserves_flags),
        );
    }
}
