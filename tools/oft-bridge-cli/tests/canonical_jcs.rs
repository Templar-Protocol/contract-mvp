//! RFC 8785 (JSON Canonicalization Scheme / JCS) golden vectors pinning the
//! crate's canonical JSON bytes and SHA-256 hashes (`canonical_sha256`).
//!
//! # Provenance
//!
//! * Vector inputs and canonical outputs for the RFC examples are taken
//!   verbatim from [RFC 8785] — Section 3.2.2 (primitive serialization
//!   example), Section 3.2.3 (property-sorting test data), and Section 3.2.4
//!   (authoritative UTF-8 byte dump of the canonical form).
//! * Every expected SHA-256 digest was independently generated in two
//!   external languages on this workstation (scripts: `/tmp/jcs_node.js`,
//!   `/tmp/jcs_py.py`); both implementations produced identical canonical
//!   bytes and identical digests:
//!   * Node.js 22.23.1 — primitives via `JSON.stringify` (the RFC 8785
//!     Appendix A sample canonicalizer semantics, i.e. ECMAScript
//!     `Number::toString` for numbers) with object keys sorted by UTF-16
//!     code units; digest via `node:crypto` sha256.
//!   * Python 3 — primitives via `json.dumps(ensure_ascii=False)` /
//!     `float.__repr__` with keys ordered as UTF-16 code units (utf-16-be
//!     byte comparison); digest via `hashlib.sha256`.
//! * The Section 3.2.4 dump was additionally cross-checked programmatically
//!   against the canonical output of both generators (match).
//!
//! `canonical_bytes` is `pub(crate)` and therefore unreachable from
//! integration tests; exact canonical bytes are pinned indirectly by
//! asserting `canonical_sha256(value)` equals the SHA-256 of the exact
//! expected byte string (collision-safe for these purposes).
//!
//! The divergence assertions in this file intentionally fail if an ordinary
//! `serde_json` serialization were substituted for the canonicalizer: plain
//! serialization either renders numbers with ryu exponent formatting (no
//! mandatory `+` sign) or orders object keys by UTF-8 bytes instead of UTF-16
//! code units, both of which change the hash.
//!
//! [RFC 8785]: https://www.rfc-editor.org/rfc/rfc8785

use serde_json::Value;
use sha2::{Digest as _, Sha256};
use templar_oft_bridge_cli::canonical_sha256;

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// RFC 8785 Section 3.2.2 example: number normalization, string escaping,
/// literals. Expected bytes are the Section 3.2.4 authoritative dump.
#[test]
fn rfc8785_322_example_pins_canonical_bytes_and_hash() {
    let input = r#"{
      "numbers": [333333333.33333329, 1E30, 4.50,
                  2e-3, 0.000000000000000000000000001],
      "string": "\u20ac$\u000F\u000aA'\u0042\u0022\u005c\\\"\/",
      "literals": [null, true, false]
    }"#;
    let value: Value = serde_json::from_str(input).expect("RFC 8785 input parses");

    // RFC 8785 Section 3.2.4 byte dump of the canonical form (spaces elided).
    let expected_hex = concat!(
        "7b226c69746572616c73223a5b6e756c6c2c747275652c66616c73655d2c",
        "226e756d62657273223a5b3333333333333333332e333333333333332c31",
        "652b33302c342e352c302e3030322c31652d32375d2c22737472696e6722",
        "3a22e282ac245c75303030665c6e4127425c225c5c5c5c5c222f227d",
    );
    let expected = hex::decode(expected_hex).expect("decode expected bytes");

    let got = canonical_sha256(&value).expect("canonical sha256");
    assert_eq!(
        got,
        digest(&expected),
        "canonical JSON bytes must equal RFC 8785 Section 3.2.4 dump"
    );
    // Independently computed golden hash (Node.js + Python, see module docs).
    assert_eq!(
        got,
        "2d5e01a318d0f0879ab568c4be289c8b1f64ef8921a53c6277d5e069978baacb"
    );
    assert_eq!(got.len(), 64);
}

/// RFC 8785 Section 3.2.3 sorting test data: property names must be ordered
/// by UTF-16 code units, which differs from code-point/UTF-8 ordering for the
/// U+1F600 (surrogate pair D83D DE00) versus U+FB33 keys.
#[test]
fn rfc8785_323_test_data_pins_utf16_key_ordering() {
    let input = r#"{
      "\u20ac": "Euro Sign",
      "\r": "Carriage Return",
      "\ufb33": "Hebrew Letter Dalet With Dagesh",
      "1": "One",
      "\ud83d\ude00": "Emoji: Grinning Face",
      "\u0080": "Control",
      "\u00f6": "Latin Small Letter O With Diaeresis"
    }"#;
    let value: Value = serde_json::from_str(input).expect("RFC 8785 input parses");

    // Canonical form (Node.js + Python agreement; keys escape per RFC 8785
    // Section 3.2.2.2: "\r" shorthand, raw UTF-8 for everything >= U+0020):
    // {"\r":"Carriage Return","1":"One","\u0080":"Control",
    //  "ö":"Latin Small Letter O With Diaeresis","€":"Euro Sign",
    //  "😀":"Emoji: Grinning Face","דּ":"Hebrew Letter Dalet With Dagesh"}
    let expected_hex = concat!(
        "7b225c72223a2243617272696167652052657475726e222c2231223a22",
        "4f6e65222c22c280223a22436f6e74726f6c222c22c3b6223a224c6174",
        "696e20536d616c6c204c6574746572204f205769746820446961657265",
        "736973222c22e282ac223a224575726f205369676e222c22f09f988022",
        "3a22456d6f6a693a204772696e6e696e672046616365222c22efacb322",
        "3a22486562726577204c65747465722044616c65742057697468204461",
        "67657368227d",
    );
    let expected = hex::decode(expected_hex).expect("decode expected bytes");

    let got = canonical_sha256(&value).expect("canonical sha256");
    assert_eq!(
        got,
        digest(&expected),
        "canonical JSON bytes must equal the UTF-16-ordered form"
    );
    // Independently computed golden hash (Node.js + Python, see module docs).
    assert_eq!(
        got,
        "5e321556d22018a9656991a9e94f77ec175fa193e52a2429d312f8419ec8b08c"
    );

    // Plain serde_json diverges: its Map is a BTreeMap ordered by UTF-8
    // bytes, i.e. code-point order, which sorts U+FB33 before U+1F600 —
    // the reverse of the JCS ordering. The two orderings hash differently.
    let plain = serde_json::to_vec(&value).expect("plain serde serializes");
    assert_ne!(
        plain, expected,
        "UTF-8-ordered serde_json bytes must differ from JCS bytes"
    );
}

/// Independently authored fixture for RFC 8785 Section 3.2.3 recursion rules:
/// child object properties are sorted recursively, including inside arrays;
/// array element order is preserved.
#[test]
fn nested_objects_and_arrays_are_sorted_recursively() {
    let input = r#"{"z":{"b":1,"a":2},"a":[{"y":true,"x":null}]}"#;
    let value: Value = serde_json::from_str(input).expect("fixture parses");

    // Canonical form (Node.js + Python agreement):
    // {"a":[{"x":null,"y":true}],"z":{"a":2,"b":1}}
    let expected_hex = concat!(
        "7b2261223a5b7b2278223a6e756c6c2c2279223a747275657d5d2c227a",
        "223a7b2261223a322c2262223a317d7d",
    );
    let expected = hex::decode(expected_hex).expect("decode expected bytes");

    let got = canonical_sha256(&value).expect("canonical sha256");
    assert_eq!(
        got,
        digest(&expected),
        "canonical JSON bytes must be recursively sorted"
    );
    // Independently computed golden hash (Node.js + Python, see module docs).
    assert_eq!(
        got,
        "f06d757e7cbd830c7ff09df6ac6728fb07b77ecb12ef0f4e4f5f26d12160c48e"
    );
}
