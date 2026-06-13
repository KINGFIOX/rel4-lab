use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let target = env::var("TARGET").unwrap();
    let linker_script = match target.as_str() {
        "loongarch64-unknown-none" => manifest_dir.join("linker-loongarch64.ld"),
        "riscv64gc-unknown-none-elf" => manifest_dir.join("linker-riscv64.ld"),
        _ => manifest_dir.join("linker-riscv64.ld"),
    };

    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=RUST_LOG");
    println!("cargo:rerun-if-env-changed=SMP");
    println!("cargo:rerun-if-env-changed=NUM_NODES");
    println!("cargo:rustc-check-cfg=cfg(kernel_smp)");
    println!(
        "cargo:rustc-check-cfg=cfg(kernel_num_nodes, values(\"1\", \"2\", \"3\", \"4\", \"5\", \"6\", \"7\", \"8\"))"
    );
    println!("cargo:rustc-env=SEL4_LOG_LEVEL={}", rust_log_level());
    let max_num_nodes = max_num_nodes();
    println!("cargo:rustc-cfg=kernel_num_nodes=\"{}\"", max_num_nodes);
    if env_bool("SMP") || max_num_nodes > 1 {
        println!("cargo:rustc-cfg=kernel_smp");
    }
    println!("cargo:rerun-if-changed={}", linker_script.display());
    println!(
        "cargo:rustc-link-arg-bin=kernel=-T{}",
        linker_script.display()
    );
    println!("cargo:rustc-link-arg-bin=kernel=--no-relax");
    println!("cargo:rustc-link-arg-bin=kernel=-zmax-page-size=4096");
}

fn env_bool(name: &str) -> bool {
    matches!(
        env::var(name)
            .unwrap_or_default()
            .trim()
            .to_ascii_uppercase()
            .as_str(),
        "1" | "ON" | "TRUE" | "YES" | "Y"
    )
}

fn max_num_nodes() -> usize {
    if let Ok(value) = env::var("NUM_NODES") {
        if let Ok(parsed) = value.trim().parse::<usize>() {
            return parsed.clamp(1, 8);
        }
    }
    if env_bool("SMP") { 2 } else { 1 }
}

fn rust_log_level() -> &'static str {
    let mut best = None;
    let value = env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    for directive in value.split(',') {
        let level = directive
            .rsplit_once('=')
            .map_or(directive, |(_, level)| level)
            .trim()
            .to_ascii_lowercase();
        let value = match level.as_str() {
            "off" => 0,
            "error" => 1,
            "warn" | "warning" => 2,
            "info" => 3,
            "debug" => 4,
            "trace" => 5,
            _ => continue,
        };
        best = Some(best.map_or(value, |best: i32| best.max(value)));
    }
    match best.unwrap_or(3) {
        0 => "off",
        1 => "error",
        2 => "warn",
        3 => "info",
        4 => "debug",
        _ => "trace",
    }
}
