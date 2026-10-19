//! Synthetic behavioral evaluation of the production lexical evidence ledger.
//! No provider, credentials, embeddings or user conversations are accessed.
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};
use uuid::Uuid;
use waifu_studio::facts_memory::{FactsMemoryState, FactsRecallOutcome, RecordKind};

fn settle(state: &mut FactsMemoryState) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        state.poll();
        assert!(
            state.error.is_none(),
            "fixture persistence error: {:?}",
            state.error
        );
        if state.loaded && !state.busy && !state.pending_saving() {
            return;
        }
        assert!(Instant::now() < deadline, "fixture worker timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn check(
    state: &FactsMemoryState,
    name: &str,
    query: &str,
    expected: &[Uuid],
) -> serde_json::Value {
    let _ = state.context(query);
    let actual: BTreeSet<_> = state
        .latest_trace()
        .into_iter()
        .flat_map(|trace| trace.hits)
        .filter(|hit| hit.outcome == FactsRecallOutcome::Inserted)
        .map(|hit| hit.record)
        .collect();
    let expected: BTreeSet<_> = expected.iter().copied().collect();
    serde_json::json!({"scenario":name,"query":query,"expected_record_count":expected.len(),"retrieved_record_count":actual.len(),"passed":actual==expected})
}
fn main() {
    let root = std::env::temp_dir().join(format!("waifu-memory-evaluation-{}", Uuid::new_v4()));
    let mut state = FactsMemoryState::load(root.clone());
    state.select_character(Some(Uuid::new_v4()));
    settle(&mut state);
    state.set_enabled(true).unwrap();
    settle(&mut state);
    state
        .add_record(
            RecordKind::Preference,
            "I prefer green tea".into(),
            "Synthetic user statement".into(),
        )
        .unwrap();
    settle(&mut state);
    let tea = state.records()[0].id;
    let mut results = vec![
        check(&state, "literal preference", "tea", &[tea]),
        check(
            &state,
            "paraphrased preference",
            "What beverage do I enjoy?",
            &[tea],
        ),
        check(&state, "unrelated query", "bicycle maintenance", &[]),
    ];
    state
        .correct_record(
            tea,
            "I now prefer coffee".into(),
            "Synthetic explicit correction".into(),
        )
        .unwrap();
    settle(&mut state);
    let coffee = state
        .records()
        .iter()
        .find(|record| record.supersedes == Some(tea))
        .unwrap()
        .id;
    results.push(check(&state, "corrected preference", "coffee", &[coffee]));
    results.push(check(&state, "superseded claim excluded", "tea", &[]));
    state.forget(coffee).unwrap();
    settle(&mut state);
    results.push(check(
        &state,
        "forget does not resurrect old preference",
        "tea coffee",
        &[],
    ));
    state
        .add_record(
            RecordKind::Fact,
            "I do not eat peanuts".into(),
            "Synthetic explicit restriction".into(),
        )
        .unwrap();
    settle(&mut state);
    let peanuts = state.records()[0].id;
    results.push(check(&state, "literal restriction", "peanuts", &[peanuts]));
    state.set_expiry(peanuts, Some(0)).unwrap();
    settle(&mut state);
    results.push(check(&state, "expired record excluded", "peanuts", &[]));
    state.select_character(Some(Uuid::new_v4()));
    settle(&mut state);
    state.set_enabled(true).unwrap();
    settle(&mut state);
    results.push(check(
        &state,
        "other character isolation",
        "tea coffee peanuts",
        &[],
    ));
    let passed = results.iter().filter(|row| row["passed"] == true).count();
    println!("{}",serde_json::to_string_pretty(&serde_json::json!({
        "evaluation":"synthetic production lexical evidence recall",
        "scope":"retrieval and ledger mutation only; no LLM understanding, episodic memory, embeddings or live acceptance",
        "passed":passed,"total":results.len(),"results":results
    })).unwrap());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
