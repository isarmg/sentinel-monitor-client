use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn validate(contract: &[u8], provenance: &[u8]) -> Result<(String, String), &'static str> {
    if contract.len() > 16384 || provenance.len() > 16384 {
        return Err("contract input exceeds limit");
    }
    let provenance: Value =
        serde_json::from_slice(provenance).map_err(|_| "invalid provenance JSON")?;
    let object = provenance
        .as_object()
        .ok_or("provenance must be an object")?;
    if object.len() != 4
        || object
            .keys()
            .any(|key| !["repository", "revision", "source_path", "sha256"].contains(&key.as_str()))
    {
        return Err("unknown provenance fields");
    }
    if provenance["repository"] != "https://github.com/isarmg/xcos.git"
        || provenance["source_path"] != "web/src/protocol-contract.json"
    {
        return Err("unexpected product authority");
    }
    let revision = provenance["revision"]
        .as_str()
        .ok_or("missing source revision")?;
    if revision.len() != 40
        || !revision
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
    {
        return Err("source revision must be a complete immutable commit");
    }
    if provenance["sha256"].as_str()
        != Some(
            Sha256::digest(contract)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
                .as_str(),
        )
    {
        return Err("contract differs from controlled source receipt");
    }
    let contract: Value = serde_json::from_slice(contract).map_err(|_| "invalid contract JSON")?;
    let object = contract.as_object().ok_or("contract must be an object")?;
    let fields = [
        "wire_protocol",
        "api_prefix",
        "media_auth_path",
        "media_jwt_protocol",
        "media_jwt_issuer",
        "media_jwt_audience",
        "media_jwt_kind",
        "edge_protocol",
    ];
    if object.len() != fields.len() || object.keys().any(|key| !fields.contains(&key.as_str())) {
        return Err("unknown contract fields");
    }
    if object.values().any(|value| {
        value.as_str().is_none_or(|value| {
            value.is_empty()
                || value.len() > 255
                || value.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
        })
    }) {
        return Err("invalid contract identifiers");
    }
    let edge = contract["edge_protocol"]
        .as_str()
        .ok_or("missing edge protocol")?;
    let prefix = contract["api_prefix"]
        .as_str()
        .ok_or("missing API prefix")?;
    if !edge.starts_with("xcos-edge-v")
        || !prefix.starts_with("/api/")
        || prefix.ends_with('/')
        || prefix.contains("..")
        || prefix.contains('?')
        || prefix.contains('#')
    {
        return Err("invalid edge contract");
    }
    Ok((edge.to_owned(), prefix.to_owned()))
}
