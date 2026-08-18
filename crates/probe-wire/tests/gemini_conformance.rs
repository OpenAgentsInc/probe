//! The shared Gemini SSE corpus (fixtures/gemini/sse-stream.json), ported
//! from the archived stream-parser tests, is this parser's acceptance suite.

use probe_wire::gemini::parse_sse;

fn fixture() -> serde_json::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/gemini/sse-stream.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Subset match: every key in `expected` must be present and equal in
/// `actual` (recursively for objects).
fn subset_matches(expected: &serde_json::Value, actual: &serde_json::Value) -> bool {
    match (expected, actual) {
        (serde_json::Value::Object(expected), serde_json::Value::Object(actual)) => expected
            .iter()
            .all(|(key, value)| actual.get(key).is_some_and(|actual| subset_matches(value, actual))),
        (expected, actual) => expected == actual,
    }
}

#[test]
fn gemini_sse_corpus_passes() {
    let corpus = fixture();
    for case in corpus["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let sse = case["sse"].as_str().unwrap();

        if let Some(expected_error) = case.get("expectedError") {
            let error = parse_sse(sse).expect_err(name);
            assert_eq!(
                error.failure_class,
                expected_error["failureClass"].as_str().unwrap(),
                "{name}: failure class"
            );
            for banned in expected_error["mustNotContain"].as_array().unwrap() {
                assert!(
                    !error.message.contains(banned.as_str().unwrap()),
                    "{name}: error message leaked payload bytes"
                );
            }
            continue;
        }

        let events = parse_sse(sse).expect(name);
        let encoded: Vec<serde_json::Value> =
            events.iter().map(|event| serde_json::to_value(event).unwrap()).collect();

        if let Some(expected_types) = case["expectedTypes"].as_array() {
            let types: Vec<&str> = encoded.iter().map(|event| event["type"].as_str().unwrap()).collect();
            let expected: Vec<&str> = expected_types.iter().map(|value| value.as_str().unwrap()).collect();
            assert_eq!(types, expected, "{name}: event type sequence");
        }

        if let Some(expectations) = case["expectations"].as_array() {
            for expectation in expectations {
                let index = expectation["index"].as_i64().unwrap();
                let actual = if index < 0 {
                    &encoded[encoded.len() - index.unsigned_abs() as usize]
                } else {
                    &encoded[index as usize]
                };
                let expected = &expectation["match"];
                if expectation["exact"].as_bool() == Some(true) {
                    assert_eq!(actual, expected, "{name}: exact expectation at {index}");
                } else {
                    assert!(
                        subset_matches(expected, actual),
                        "{name}: subset expectation at {index}\nexpected subset: {expected}\nactual: {actual}"
                    );
                }
            }
        }
    }
}
