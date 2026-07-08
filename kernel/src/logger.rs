use core::fmt::Write;

use log_crate::{LevelFilter, Log, Metadata, Record};

use crate::print::KConsole;

static LOGGER: KernelLogger = KernelLogger;

struct KernelLogger;

pub fn init() {
    let _ = log_crate::set_logger(&LOGGER);
    log_crate::set_max_level(max_level());
}

impl Log for KernelLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log_crate::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let mut console = KConsole;
            let _ = console.write_fmt(*record.args());
            let _ = console.write_str("\n");
        }
    }

    fn flush(&self) {}
}

fn max_level() -> LevelFilter {
    match env!("SEL4_LOG_LEVEL") {
        "off" => LevelFilter::Off,
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Info,
    }
}
