//! Kernel print macros routed through `machine::console`.

use core::fmt::{self, Write};

use crate::machine::console;

pub struct KConsole;

impl Write for KConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            if b == b'\n' {
                console::putc(b'\r');
            }
            console::putc(b);
        }
        Ok(())
    }
}

pub fn _print(args: fmt::Arguments<'_>) {
    let _ = KConsole.write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::print::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
