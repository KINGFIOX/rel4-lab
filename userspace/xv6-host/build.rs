use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let linker_script = manifest_dir.join("linker.ld");

    println!("cargo:rerun-if-changed={}", linker_script.display());
    println!(
        "cargo:rustc-link-arg-bin=xv6-host=-T{}",
        linker_script.display()
    );
    println!("cargo:rustc-link-arg-bin=xv6-host=--no-relax");
    println!("cargo:rustc-link-arg-bin=xv6-host=-zmax-page-size=4096");

    println!("cargo:rerun-if-env-changed=XV6_PAYLOAD_ELF");
    println!("cargo:rerun-if-env-changed=XV6_EXEC_CATALOG_RS");
    println!("cargo:rerun-if-env-changed=XV6_CONSOLE_INPUT");
    let payload = env::var("XV6_PAYLOAD_ELF").unwrap_or_else(|_| {
        panic!(
            "XV6_PAYLOAD_ELF must point to a linked xv6 user payload; \
             use tools/build-xv6-user-rootserver.sh PROGRAM [ARG...]"
        )
    });
    println!("cargo:rerun-if-changed={payload}");
    println!("cargo:rustc-env=XV6_PAYLOAD_ELF={payload}");

    let catalog = env::var("XV6_EXEC_CATALOG_RS").unwrap_or_else(|_| {
        panic!(
            "XV6_EXEC_CATALOG_RS must point to the generated xv6 exec catalog; \
             use tools/build-xv6-user-rootserver.sh PROGRAM [ARG...]"
        )
    });
    println!("cargo:rerun-if-changed={catalog}");
    println!("cargo:rustc-env=XV6_EXEC_CATALOG_RS={catalog}");
}
