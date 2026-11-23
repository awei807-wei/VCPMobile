use serde_json::Value;
use sha2::{Digest, Sha256};

const WIRE_ERROR_FIXTURE_SHA256: &str =
    "b823f87d6a89ebefe6b173635d8e2c3ea9918f3f0415622020e1da80d6547957";
const MESSAGE_DIFF_FIXTURE_SHA256: &str =
    "abe599e454de49ca12f9faed78fef954ab013f4031c90b4f87fd8fc7ee37ae28";
const MESSAGE_CANONICAL_FIXTURE_SHA256: &str =
    "d7e7303371db5454fd27fe1acce0830d49db6d47c5230cdd29411c2263b144da";
const TOPIC_CANONICAL_FIXTURE_SHA256: &str =
    "eea19058270131fc42c3dfd1dfff80972efe42d3df769d52ecff285b6beea615";
const VERSION_HANDSHAKE_FIXTURE_SHA256: &str =
    "0fa6cec25e7b878aea5e135abfc38d54b24d9140acf719a55416497b857ae895";

fn sha256(bytes: &[u8]) -> String {
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'\r' && bytes.get(idx + 1) == Some(&b'\n') {
            idx += 1;
            continue;
        }
        normalized.push(bytes[idx]);
        idx += 1;
    }
    format!("{:x}", Sha256::digest(&normalized))
}
#[test]
fn current_desktop_fixtures_are_byte_exact() {
    assert_eq!(
        sha256(include_bytes!("../fixtures/wire_error_contract.json")),
        WIRE_ERROR_FIXTURE_SHA256
    );
    assert_eq!(
        sha256(include_bytes!("../fixtures/message_diff_matrix.json")),
        MESSAGE_DIFF_FIXTURE_SHA256
    );
    assert_eq!(
        sha256(include_bytes!(
            "../fixtures/message_canonical_contract.json"
        )),
        MESSAGE_CANONICAL_FIXTURE_SHA256
    );
    assert_eq!(
        sha256(include_bytes!("../fixtures/topic_canonical_contract.json")),
        TOPIC_CANONICAL_FIXTURE_SHA256
    );
    assert_eq!(
        sha256(include_bytes!(
            "../fixtures/version_handshake_contract.json"
        )),
        VERSION_HANDSHAKE_FIXTURE_SHA256
    );
}

#[test]
fn current_desktop_fixtures_are_wire_1_6_contracts() {
    let error: Value =
        serde_json::from_slice(include_bytes!("../fixtures/wire_error_contract.json"))
            .expect("wire error fixture JSON");
    let canonical: Value = serde_json::from_slice(include_bytes!(
        "../fixtures/message_canonical_contract.json"
    ))
    .expect("canonical fixture JSON");
    let diff: Value =
        serde_json::from_slice(include_bytes!("../fixtures/message_diff_matrix.json"))
            .expect("diff fixture JSON");
    let topic: Value =
        serde_json::from_slice(include_bytes!("../fixtures/topic_canonical_contract.json"))
            .expect("topic fixture JSON");
    let version: Value = serde_json::from_slice(include_bytes!(
        "../fixtures/version_handshake_contract.json"
    ))
    .expect("version fixture JSON");

    assert_eq!(error["schema"], "vcp-sync-wire-error-contract");
    assert_eq!(canonical["schema"], "vcp-sync-message-canonical-contract");
    assert_eq!(diff["schema"], "vcp-sync-message-diff-matrix");
    assert_eq!(topic["schema"], "vcp-sync-topic-canonical-contract");
    assert_eq!(version["schema"], "vcp-sync-version-handshake-contract");
    assert_eq!(version["wireVersion"], "1.6");
    assert!(canonical["validFrames"].as_array().is_some());
    assert!(diff["cases"].as_array().is_some());
    assert!(topic["cases"].as_array().is_some());
}
