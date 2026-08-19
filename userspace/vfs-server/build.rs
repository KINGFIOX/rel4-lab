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
    println!("cargo:rerun-if-env-changed=LINUX_ROOTFS_CPIO");
    println!("cargo:rerun-if-env-changed=LINUX_CONSOLE_INPUT");
    println!("cargo:rerun-if-changed={}", linker_script.display());
    println!(
        "cargo:rustc-link-arg-bin=vfs-server=-T{}",
        linker_script.display()
    );
    println!("cargo:rustc-link-arg-bin=vfs-server=--no-relax");
    println!("cargo:rustc-link-arg-bin=vfs-server=-zmax-page-size=4096");

    let rootfs = match env::var("LINUX_ROOTFS_CPIO") {
        Ok(path) => PathBuf::from(path),
        Err(_) if allow_placeholders => {
            let placeholder = out_dir.join("empty-rootfs.cpio");
            if !placeholder.is_file() {
                fs::write(&placeholder, empty_cpio()).unwrap();
            }
            placeholder
        }
        Err(_) => {
            panic!("LINUX_ROOTFS_CPIO must point to a newc cpio; use tools/build-linux-rootfs.py")
        }
    };
    println!("cargo:rerun-if-changed={}", rootfs.display());
    println!("cargo:rustc-env=LINUX_ROOTFS_CPIO={}", rootfs.display());
}

fn linker_script_for_target(manifest_dir: &PathBuf) -> PathBuf {
    let target = env::var("TARGET").unwrap();
    let filename = match target.as_str() {
        "riscv64gc-unknown-none-elf" => "linker-riscv64.ld",
        _ => panic!("unsupported target for vfs-server: {target}"),
    };
    manifest_dir.join(filename)
}

fn empty_cpio() -> Vec<u8> {
    // Minimal newc trailer.
    let name = b"TRAILER!!!\0";
    let mut header = [0u8; 110];
    header[..6].copy_from_slice(b"070701");
    write_hex(&mut header[94..102], name.len() as u32);
    let mut out = Vec::from(header);
    out.extend_from_slice(name);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

fn write_hex(dst: &mut [u8], value: u32) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut v = value;
    for i in (0..8).rev() {
        dst[i] = HEX[(v & 0xf) as usize];
        v >>= 4;
    }
}
