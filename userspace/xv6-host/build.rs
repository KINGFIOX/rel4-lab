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
    println!("cargo:rerun-if-env-changed=XV6_FS_SERVER_ELF");
    println!("cargo:rerun-if-env-changed=XV6_DISK_SERVER_ELF");
    println!("cargo:rerun-if-env-changed=XV6_EXEC_CATALOG_RS");
    println!("cargo:rerun-if-env-changed=XV6_CONSOLE_INPUT");
    println!("cargo:rerun-if-env-changed=XV6_PAYLOAD_PROGRAM");
    println!("cargo:rerun-if-env-changed=XV6_ROOT_IS_INIT");
    let payload = env::var("XV6_PAYLOAD_ELF").unwrap_or_else(|_| {
        panic!(
            "XV6_PAYLOAD_ELF must point to a linked xv6 user payload; \
             use tools/build-xv6-user-rootserver.sh PROGRAM [ARG...]"
        )
    });
    println!("cargo:rerun-if-changed={payload}");
    println!("cargo:rustc-env=XV6_PAYLOAD_ELF={payload}");

    let fs_server = env::var("XV6_FS_SERVER_ELF").unwrap_or_else(|_| {
        panic!(
            "XV6_FS_SERVER_ELF must point to the xv6-fs-server ELF; \
             use tools/build-xv6-user-rootserver.sh PROGRAM [ARG...]"
        )
    });
    println!("cargo:rerun-if-changed={fs_server}");
    println!("cargo:rustc-env=XV6_FS_SERVER_ELF={fs_server}");

    let disk_server = env::var("XV6_DISK_SERVER_ELF").unwrap_or_else(|_| {
        panic!(
            "XV6_DISK_SERVER_ELF must point to the virtio-disk-server ELF; \
             use tools/build-xv6-user-rootserver.sh PROGRAM [ARG...]"
        )
    });
    println!("cargo:rerun-if-changed={disk_server}");
    println!("cargo:rustc-env=XV6_DISK_SERVER_ELF={disk_server}");

    let catalog = env::var("XV6_EXEC_CATALOG_RS").unwrap_or_else(|_| {
        panic!(
            "XV6_EXEC_CATALOG_RS must point to the generated xv6 exec catalog; \
             use tools/build-xv6-user-rootserver.sh PROGRAM [ARG...]"
        )
    });
    println!("cargo:rerun-if-changed={catalog}");
    println!("cargo:rustc-env=XV6_EXEC_CATALOG_RS={catalog}");

    if let Ok(program) = env::var("XV6_PAYLOAD_PROGRAM") {
        println!("cargo:rustc-env=XV6_PAYLOAD_PROGRAM={program}");
    }
    if env::var("XV6_ROOT_IS_INIT").as_deref() == Ok("1") {
        println!("cargo:rustc-env=XV6_COMPILED_ROOT_IS_INIT=1");
    }
}
