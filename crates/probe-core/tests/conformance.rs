//! Shared-fixture conformance: every value in fixtures/llm must decode into
//! the canonical Rust types and re-encode to exactly the same JSON. The TS
//! mirror in @openagentsinc/probe runs the same corpus (Phase 6). The Gemini
//! corpus in fixtures/gemini is consumed by probe-wire (Phase 3).

use probe_core::contract::event::Event;
use probe_core::contract::message::Message;
use probe_core::contract::request::Request;
use probe_core::contract::usage::Usage;
use probe_core::editing::{plan_exact_edit, EditError};

fn fixture(path: &str) -> serde_json::Value {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/");
    let raw = std::fs::read_to_string(format!("{root}{path}")).expect("fixture file");
    serde_json::from_str(&raw).expect("fixture JSON")
}

fn assert_roundtrip<T>(value: &serde_json::Value, context: &str)
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let decoded: T = serde_json::from_value(value.clone())
        .unwrap_or_else(|error| panic!("{context}: failed to decode: {error}\n{value}"));
    let encoded = serde_json::to_value(&decoded).expect("encode");
    assert_eq!(&encoded, value, "{context}: re-encoded JSON diverged");
}

#[test]
fn contract_values_round_trip_byte_for_byte() {
    let corpus = fixture("llm/roundtrip.json");
    for (index, value) in corpus["events"].as_array().unwrap().iter().enumerate() {
        assert_roundtrip::<Event>(value, &format!("events[{index}]"));
    }
    for (index, value) in corpus["messages"].as_array().unwrap().iter().enumerate() {
        assert_roundtrip::<Message>(value, &format!("messages[{index}]"));
    }
    for (index, value) in corpus["requests"].as_array().unwrap().iter().enumerate() {
        assert_roundtrip::<Request>(value, &format!("requests[{index}]"));
    }
    for (index, value) in corpus["usages"].as_array().unwrap().iter().enumerate() {
        assert_roundtrip::<Usage>(value, &format!("usages[{index}]"));
    }
}

#[test]
fn usage_normalization_matches_the_corpus() {
    let corpus = fixture("llm/usage-normalization.json");
    for case in corpus["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let input: Usage = serde_json::from_value(case["input"].clone()).expect(name);
        let expected: Usage = serde_json::from_value(case["expected"].clone()).expect(name);
        let normalized = input.normalized();
        assert_eq!(normalized, expected, "{name}");
        if let Some(visible) = case["visibleOutputTokens"].as_u64() {
            assert_eq!(normalized.visible_output_tokens(), visible, "{name}: visible output");
        }
    }
}

#[test]
fn edit_policy_matches_the_corpus() {
    let corpus = fixture("llm/edit-policy.json");
    for case in corpus["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let outcome = plan_exact_edit(
            case["content"].as_str().unwrap(),
            case["oldString"].as_str().unwrap(),
            case["newString"].as_str().unwrap(),
            case["replaceAll"].as_bool().unwrap(),
        );
        match (outcome, case.get("expected"), case.get("error")) {
            (Ok(plan), Some(expected), _) => {
                assert_eq!(plan.output, expected["output"].as_str().unwrap(), "{name}: output");
                assert_eq!(
                    plan.replacements as u64,
                    expected["replacements"].as_u64().unwrap(),
                    "{name}: replacements"
                );
            }
            (Err(error), _, Some(expected_error)) => {
                let tag = match error {
                    EditError::EmptyOldString => "empty_old_string",
                    EditError::NoMatch => "no_match",
                    EditError::AmbiguousMatch { .. } => "ambiguous_match",
                };
                assert_eq!(tag, expected_error.as_str().unwrap(), "{name}: error class");
            }
            (outcome, _, _) => panic!("{name}: unexpected outcome {outcome:?}"),
        }
    }
}

#[test]
fn gemini_corpus_is_well_formed_for_phase_3() {
    // probe-wire consumes this corpus in Phase 3 (#209); until then, keep it
    // parseable and its expected neutral events decodable, so drift is caught
    // where the fixtures live, not months later.
    let corpus = fixture("gemini/sse-stream.json");
    for case in corpus["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        assert!(case["sse"].is_string(), "{name}: sse payload");
        if let Some(expectations) = case["expectations"].as_array() {
            for expectation in expectations {
                if expectation["exact"].as_bool() == Some(true) {
                    assert_roundtrip::<Event>(&expectation["match"], &format!("gemini `{name}` exact expectation"));
                }
            }
        }
    }
}
