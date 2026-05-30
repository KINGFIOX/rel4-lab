use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let linker_script = manifest_dir.join("linker.ld");

    println!("cargo:rerun-if-changed={}", linker_script.display());
    println!(
        "cargo:rustc-link-arg-bin=virtio-disk-server=-T{}",
        linker_script.display()
    );
    println!("cargo:rustc-link-arg-bin=virtio-disk-server=--no-relax");
    println!("cargo:rustc-link-arg-bin=virtio-disk-server=-zmax-page-size=4096");
    println!("cargo:rerun-if-env-changed=XV6_TRACE_BLOCK_IO");
}
