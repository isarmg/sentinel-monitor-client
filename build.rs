#[path = "protocol/build_contract.rs"]
mod contract;
fn main() {
    build_media();
    println!("cargo:rerun-if-changed=protocol/protocol-contract.json");
    println!("cargo:rerun-if-changed=protocol/provenance.json");
    println!("cargo:rerun-if-changed=protocol/build_contract.rs");
    let (edge, prefix) = contract::validate(
        include_bytes!("protocol/protocol-contract.json"),
        include_bytes!("protocol/provenance.json"),
    )
    .expect("controlled Xcoc product protocol contract is invalid");
    println!("cargo:rustc-env=XCOC_EDGE_PROTOCOL={edge}");
    println!("cargo:rustc-env=XCOC_API_PREFIX={prefix}");
}

fn build_media() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("target OS");
    if matches!(target_os.as_str(), "android" | "ios") {
        return;
    }
    println!("cargo:rerun-if-changed=native/media.c");
    println!("cargo:rerun-if-changed=native/media.h");
    let mut compiler = cc::Build::new();
    compiler.file("native/media.c").std("c11").warnings(true);
    let mut libraries = Vec::new();
    for (name, minimum) in [
        ("libavformat", "63.1.102"),
        ("libavcodec", "63.1.102"),
        ("libavutil", "61.1.102"),
    ] {
        let library = pkg_config::Config::new()
            .atleast_version(minimum)
            .statik(true)
            .cargo_metadata(false)
            .probe(name)
            .expect(
                "FFmpeg 9.0.2 static development libraries are required; see docs/media-worker.md",
            );
        let archive = if target_os == "windows" {
            format!("{}.lib", name.trim_start_matches("lib"))
        } else {
            format!("{name}.a")
        };
        assert!(
            library
                .link_paths
                .iter()
                .any(|path| path.join(&archive).is_file()),
            "FFmpeg static archive is missing: {archive}"
        );
        for path in &library.link_paths {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        compiler.includes(&library.include_paths);
        libraries.push(name);
    }
    compiler.compile("xcoc_media");
    // Native link order matters: our shim precedes FFmpeg and its TLS dependencies.
    for name in libraries {
        pkg_config::Config::new()
            .statik(true)
            .probe(name)
            .expect("checked FFmpeg library");
    }
}
