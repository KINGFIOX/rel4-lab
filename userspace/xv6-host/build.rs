use std::env;
use std::fs;
use std::path::PathBuf;

const RISCV_ELF_MACHINE: u16 = 243;
const LOONGARCH64_ELF_MACHINE: u16 = 258;
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
        "cargo:rustc-link-arg-bin=xv6-host=-T{}",
        linker_script.display()
    );
    println!("cargo:rustc-link-arg-bin=xv6-host=--no-relax");
    println!("cargo:rustc-link-arg-bin=xv6-host=-zmax-page-size=4096");

    println!("cargo:rerun-if-env-changed=XV6_PAYLOAD_ELF");
    println!("cargo:rerun-if-env-changed=XV6_UART_SERVER_ELF");
    println!("cargo:rerun-if-env-changed=XV6_VFS_SERVER_ELF");
    println!("cargo:rerun-if-env-changed=XV6_XV6FS_SERVER_ELF");
    println!("cargo:rerun-if-env-changed=XV6_DISK_SERVER_ELF");
    println!("cargo:rerun-if-env-changed=XV6_PAYLOAD_PROGRAM");
    println!("cargo:rerun-if-env-changed=XV6_ROOT_IS_INIT");
    let payload = resolve_embedded_elf(
        "XV6_PAYLOAD_ELF",
        &out_dir,
        allow_placeholders,
        "payload-elf",
        "linked xv6 user payload",
        "tools/build-xv6-user-rootserver.py PROGRAM [ARG...]",
    );
    let uart_server = resolve_embedded_elf(
        "XV6_UART_SERVER_ELF",
        &out_dir,
        allow_placeholders,
        "uart-server-elf",
        "uart-server ELF",
        "tools/build-xv6-user-rootserver.py PROGRAM [ARG...]",
    );
    let vfs_server = resolve_embedded_elf(
        "XV6_VFS_SERVER_ELF",
        &out_dir,
        allow_placeholders,
        "vfs-server-elf",
        "vfs-server ELF",
        "tools/build-xv6-user-rootserver.py PROGRAM [ARG...]",
    );
    let xv6fs_server = resolve_embedded_elf(
        "XV6_XV6FS_SERVER_ELF",
        &out_dir,
        allow_placeholders,
        "xv6fs-server-elf",
        "xv6fs-server ELF",
        "tools/build-xv6-user-rootserver.py PROGRAM [ARG...]",
    );
    let disk_server = resolve_embedded_elf(
        "XV6_DISK_SERVER_ELF",
        &out_dir,
        allow_placeholders,
        "disk-server-elf",
        "virtio-disk-server ELF",
        "tools/build-xv6-user-rootserver.py PROGRAM [ARG...]",
    );
    validate_embedded_elf(&payload, "payload ELF", allow_placeholders);
    validate_embedded_elf(&uart_server, "uart-server ELF", allow_placeholders);
    validate_embedded_elf(&vfs_server, "vfs-server ELF", allow_placeholders);
    validate_embedded_elf(&xv6fs_server, "xv6fs-server ELF", allow_placeholders);
    validate_embedded_elf(&disk_server, "virtio-disk-server ELF", allow_placeholders);
    println!("cargo:rerun-if-changed={}", payload.display());
    println!("cargo:rerun-if-changed={}", uart_server.display());
    println!("cargo:rerun-if-changed={}", vfs_server.display());
    println!("cargo:rerun-if-changed={}", xv6fs_server.display());
    println!("cargo:rerun-if-changed={}", disk_server.display());
    println!("cargo:rustc-env=XV6_PAYLOAD_ELF={}", payload.display());
    println!(
        "cargo:rustc-env=XV6_UART_SERVER_ELF={}",
        uart_server.display()
    );
    println!(
        "cargo:rustc-env=XV6_VFS_SERVER_ELF={}",
        vfs_server.display()
    );
    println!(
        "cargo:rustc-env=XV6_XV6FS_SERVER_ELF={}",
        xv6fs_server.display()
    );
    println!(
        "cargo:rustc-env=XV6_DISK_SERVER_ELF={}",
        disk_server.display()
    );

    if let Ok(program) = env::var("XV6_PAYLOAD_PROGRAM") {
        println!("cargo:rustc-env=XV6_PAYLOAD_PROGRAM={program}");
    }
    if env::var("XV6_ROOT_IS_INIT").as_deref() == Ok("1") {
        println!("cargo:rustc-env=XV6_COMPILED_ROOT_IS_INIT=1");
    }
}

fn linker_script_for_target(manifest_dir: &PathBuf) -> PathBuf {
    let target = env::var("TARGET").unwrap();
    let filename = match target.as_str() {
        "riscv64gc-unknown-none-elf" => "linker-riscv64.ld",
        "loongarch64-unknown-none" => "linker-loongarch64.ld",
        _ => panic!("unsupported target for xv6-host: {target}"),
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
            let placeholder_dir = out_dir.join("xv6-host-placeholders");
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
        "loongarch64-unknown-none" => LOONGARCH64_ELF_MACHINE,
        _ => panic!("unsupported target for xv6-host: {target}"),
    }
}
