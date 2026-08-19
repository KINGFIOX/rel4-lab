use std::env;
use std::fs;
use std::path::PathBuf;

const RISCV_ELF_MACHINE: u16 = 243;
const ELF_TYPE_EXECUTABLE: u16 = 2;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let linker_script = linker_script_for_target(&manifest_dir);
    let profile = env::var("PROFILE").unwrap_or_default();
    let allow_placeholders =
        profile != "release" && env::var_os("CARGO_CFG_DEBUG_ASSERTIONS").is_some();

    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-changed={}", linker_script.display());
    println!(
        "cargo:rustc-link-arg-bin=linux-compat=-T{}",
        linker_script.display()
    );
    println!("cargo:rustc-link-arg-bin=linux-compat=--no-relax");
    println!("cargo:rustc-link-arg-bin=linux-compat=-zmax-page-size=4096");

    println!("cargo:rerun-if-env-changed=LINUX_UART_SERVER_ELF");
    println!("cargo:rerun-if-env-changed=LINUX_VFS_SERVER_ELF");

    let uart_server = resolve_embedded_elf(
        "LINUX_UART_SERVER_ELF",
        &out_dir,
        allow_placeholders,
        "uart-server-elf",
        "uart-server ELF",
        "tools/build-linux-rootfs.py",
    );
    let vfs_server = resolve_embedded_elf(
        "LINUX_VFS_SERVER_ELF",
        &out_dir,
        allow_placeholders,
        "vfs-server-elf",
        "vfs-server ELF",
        "tools/build-linux-rootfs.py",
    );
    validate_embedded_elf(&uart_server, "uart-server ELF", allow_placeholders);
    validate_embedded_elf(&vfs_server, "vfs-server ELF", allow_placeholders);
    println!("cargo:rerun-if-changed={}", uart_server.display());
    println!("cargo:rerun-if-changed={}", vfs_server.display());
    println!(
        "cargo:rustc-env=LINUX_UART_SERVER_ELF={}",
        uart_server.display()
    );
    println!(
        "cargo:rustc-env=LINUX_VFS_SERVER_ELF={}",
        vfs_server.display()
    );
}

fn linker_script_for_target(manifest_dir: &PathBuf) -> PathBuf {
    let target = env::var("TARGET").unwrap();
    let filename = match target.as_str() {
        "riscv64gc-unknown-none-elf" => "linker-riscv64.ld",
        _ => panic!("unsupported target for linux-compat: {target}"),
    };
    manifest_dir.join(filename)
}

fn resolve_embedded_elf(
    var: &str,
    out_dir: &PathBuf,
    allow_placeholders: bool,
    placeholder_name: &str,
    purpose: &str,
    hint: &str,
) -> PathBuf {
    match env::var(var) {
        Ok(path) => PathBuf::from(path),
        Err(_) if allow_placeholders => {
            let placeholder_dir = out_dir.join("linux-compat-placeholders");
            let placeholder = placeholder_dir.join(format!("{placeholder_name}.elf"));
            fs::create_dir_all(&placeholder_dir).unwrap();
            if !placeholder.is_file() {
                fs::write(&placeholder, []).unwrap();
            }
            placeholder
        }
        Err(_) => panic!("{var} must point to a {purpose}; use {hint}"),
    }
}

fn validate_embedded_elf(path: &PathBuf, purpose: &str, allow_placeholders: bool) {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => panic!("{purpose} not found: {}", path.display()),
    };
    if allow_placeholders && metadata.len() == 0 {
        return;
    }
    let data = fs::read(path).unwrap();
    if data.len() < 64 || &data[0..4] != b"\x7fELF" || data[4] != 2 || data[5] != 1 {
        panic!(
            "expected a little-endian ELF64 {purpose}: {}",
            path.display()
        );
    }
    let expected_machine = expected_machine_for_target();
    let elf_type = u16::from_le_bytes([data[16], data[17]]);
    let machine = u16::from_le_bytes([data[18], data[19]]);
    if elf_type != ELF_TYPE_EXECUTABLE || machine != expected_machine {
        panic!(
            "expected an executable {purpose} for target {expected_machine:#x}: {} has e_type={elf_type:#x} e_machine={machine:#x}",
            path.display(),
        );
    }
}

fn expected_machine_for_target() -> u16 {
    let target = env::var("TARGET").unwrap();
    match target.as_str() {
        "riscv64gc-unknown-none-elf" => RISCV_ELF_MACHINE,
        _ => panic!("unsupported target for linux-compat: {target}"),
    }
}
