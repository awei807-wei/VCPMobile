use serde_json::Value;
use sha2::{Digest, Sha256};

const WIRE_ERROR_FIXTURE_SHA256: &str =
    "3a4085b0859c6dbb3b8ebbcff4db3586c890ffe624ea28f8b2d54d362b04dc2c";
const MESSAGE_DIFF_FIXTURE_SHA256: &str =
    "d30089c1fcc252c88f8b87e462a349cd9ac68c6022405b56854904f7c24749b1";
const MESSAGE_CANONICAL_FIXTURE_SHA256: &str =
    "b8e2246d6eafc36590ff42b7db0ad93ca761c6b8f94f219829f8da74e3001d51";

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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
}

#[test]
fn current_desktop_fixtures_are_wire_1_4_contracts() {
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

    assert_eq!(error["schema"], "vcp-sync-wire-error-contract");
    assert_eq!(canonical["schema"], "vcp-sync-message-canonical-contract");
    assert_eq!(diff["schema"], "vcp-sync-message-diff-matrix");
    assert!(canonical["validFrames"].as_array().is_some());
    assert!(diff["cases"].as_array().is_some());
}
