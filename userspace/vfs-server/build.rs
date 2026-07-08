use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let linker_script = linker_script_for_target(&manifest_dir);

    println!("cargo:rerun-if-env-changed=XV6_CONSOLE_INPUT");
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-changed={}", linker_script.display());
    println!(
        "cargo:rustc-link-arg-bin=vfs-server=-T{}",
        linker_script.display()
    );
    println!("cargo:rustc-link-arg-bin=vfs-server=--no-relax");
    println!("cargo:rustc-link-arg-bin=vfs-server=-zmax-page-size=4096");
}

fn linker_script_for_target(manifest_dir: &PathBuf) -> PathBuf {
    let target = env::var("TARGET").unwrap();
    let filename = match target.as_str() {
        "riscv64gc-unknown-none-elf" => "linker-riscv64.ld",
        "loongarch64-unknown-none" => "linker-loongarch64.ld",
        _ => panic!("unsupported target for vfs-server: {target}"),
    };
    manifest_dir.join(filename)
}
