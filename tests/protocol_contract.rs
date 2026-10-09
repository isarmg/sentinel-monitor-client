#[path = "../protocol/build_contract.rs"]
mod contract;
const SOURCE: &[u8] = include_bytes!("../protocol/protocol-contract.json");
const RECEIPT: &[u8] = include_bytes!("../protocol/provenance.json");

#[test]
fn compiled_wire_values_come_from_the_controlled_product_contract() {
    let (edge, prefix) = contract::validate(SOURCE, RECEIPT).unwrap();
    assert_eq!(env!("XCOC_EDGE_PROTOCOL"), edge);
    assert_eq!(env!("XCOC_API_PREFIX"), prefix);
}

#[test]
fn modified_contract_or_unknown_provenance_cannot_generate_a_wire_value() {
    let mut modified = SOURCE.to_vec();
    modified.push(b' ');
    assert!(contract::validate(&modified, RECEIPT).is_err());
    let mut provenance: serde_json::Value = serde_json::from_slice(RECEIPT).unwrap();
    provenance["revision"] = "main".into();
    assert!(contract::validate(SOURCE, &serde_json::to_vec(&provenance).unwrap()).is_err());
    provenance = serde_json::from_slice(RECEIPT).unwrap();
    provenance["repository"] = "https://example.invalid/copied".into();
    assert!(contract::validate(SOURCE, &serde_json::to_vec(&provenance).unwrap()).is_err());
    provenance = serde_json::from_slice(RECEIPT).unwrap();
    provenance["fallback"] = "old protocol".into();
    assert!(contract::validate(SOURCE, &serde_json::to_vec(&provenance).unwrap()).is_err());
}
