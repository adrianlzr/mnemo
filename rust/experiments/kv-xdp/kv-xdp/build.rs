use std::{env, path::PathBuf, process::Command};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ebpf_dir = manifest_dir.join("../kv-xdp-ebpf");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed={}", ebpf_dir.join("src").display());

    // Requires: rustup component add rust-src --toolchain nightly
    //           cargo install bpf-linker
    let status = Command::new("cargo")
        .current_dir(&ebpf_dir)
        .args([
            "+nightly",
            "build",
            "--release",
            "-Z",
            "build-std=core",
            "--target",
            "bpfel-unknown-none",
        ])
        .status()
        .expect("failed to run cargo build for kv-xdp-ebpf");

    if !status.success() {
        panic!("eBPF build failed");
    }

    let built = ebpf_dir.join("target/bpfel-unknown-none/release/kv-xdp");
    std::fs::copy(&built, out_dir.join("kv-xdp"))
        .unwrap_or_else(|e| panic!("failed to copy {}: {e}", built.display()));
}
