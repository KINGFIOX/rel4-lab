use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=RUST_LOG");
    println!("cargo:rustc-env=SEL4_LOG_LEVEL={}", rust_log_level());
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
