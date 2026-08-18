//! Shared-fixture conformance for the event -> session/update mapping.

use probe_core::contract::event::Event;
use probe_core::permission::ToolKind;
use probe_acp::mapping::updates_for_event;

fn fixture(path: &str) -> serde_json::Value {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/");
    let raw = std::fs::read_to_string(format!("{root}{path}")).expect("fixture file");
    serde_json::from_str(&raw).expect("fixture JSON")
}

#[test]
fn event_mapping_matches_the_corpus() {
    let corpus = fixture("acp/event-mapping.json");
    for case in corpus["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let event: Event = serde_json::from_value(case["event"].clone()).expect(name);
        let kinds = case["toolKinds"].as_object().unwrap().clone();
        let kind_for = move |tool: &str| -> ToolKind {
            kinds
                .get(tool)
                .and_then(|kind| serde_json::from_value::<ToolKind>(kind.clone()).ok())
                .unwrap_or(ToolKind::Other)
        };
        let budget = case["textBudget"].as_u64().unwrap() as usize;
        let updates = updates_for_event(&event, &kind_for, budget);
        let encoded = serde_json::to_value(&updates).unwrap();
        assert_eq!(encoded, case["expectedUpdates"], "{name}");
    }
}
