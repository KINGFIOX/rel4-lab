use std::env;
use std::fs;
use std::path::PathBuf;

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
