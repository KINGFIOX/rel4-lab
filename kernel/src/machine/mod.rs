pub mod console;
// `uart` is kept around for reference / standalone M-mode use but is not
// linked into the S-mode kernel image.
#[allow(dead_code)]
pub mod uart;
