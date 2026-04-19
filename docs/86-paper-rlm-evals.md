# Paper RLM Evals

## Purpose

`probe-core::issue_thread_eval` is the paper-RLM acceptance and comparison
layer above `probe-core::issue_thread_analysis`.

It snapshots one issue-thread corpus, runs both:

- `issue_thread_direct_v1`
- `paper_rlm_issue_thread_v1`

and then emits one retained comparison report with:

- task success
- evidence checks
- iteration count
- sub-LM count
- latency
- prompt-char cost proxy
- artifact refs
- controller-history externalization checks

## What It Checks

The current report asserts:

- the corpus snapshot really contains the expected number of items
- the paper RLM lane made the expected recursive sub-LM calls
- required answer substrings and evidence refs are present when configured
- `trajectory.json` and `subcall_receipts.json` were retained
- the controller history did not silently inline issue-body or late-comment
  corpus excerpts

## Current Regression Surface

Synthetic acceptance:

```bash
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo test -p probe-core \
  comparison_report_requires_externalization_and_recursive_subcalls \
  -- --nocapture
```

That case forces:

- a long synthetic issue-thread corpus
- multiple controller iterations
- multiple recursive sub-LM calls
- a final answer that depends on the late corpus region

Gated live `OpenAgentsInc/openagents#4368` comparison:

```bash
GH_TOKEN=... PROBE_OPENAI_API_KEY=... \
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo test -p probe-core \
  live_openagents_4368_comparison_reads_the_full_current_thread \
  -- --ignored --nocapture
```

Optional environment:

- `PROBE_LIVE_RLM_MODEL`
  - defaults to `gpt-4.1-mini`

The live case fetches the current full issue body plus all comments, requires a
minimum corpus size floor, runs both direct and paper-RLM strategies over that
same snapshot, and checks that the paper-RLM controller history stayed
externalized while still returning grounded evidence refs.
