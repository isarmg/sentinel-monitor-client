#[path = "protocol/build_contract.rs"]
mod contract;
fn main() {
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
