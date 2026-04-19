# Issue-Thread Strategy Routing

## Purpose

Probe now has an explicit issue-thread analysis lane in
`probe-core::issue_thread_analysis`.

That lane exists so a selected GitHub issue thread can stay a typed corpus
handle all the way through strategy routing instead of getting flattened into
prompt glue by default.

## Strategy Surface

The route decision is typed in `heuristic_rlm_trigger_v1`:

- `direct`
  - single-shot direct issue-thread analysis
- `rlm`
  - paper-style recursive language-model execution
- `compact`
  - reserved "stay out of issue-thread execution until the corpus handle exists"
    decision for long-context planning

The current operator override is:

- CLI: `--strategy auto|direct|rlm`
- TUI: `/rlm auto|on|off|status`

Normal coding turns still stay on the direct lane unless the operator forces
RLM or the selected issue-thread route triggers it.

## CLI

Dry-run the route decision and corpus receipt:

```bash
cargo run -p probe-cli -- forge rlm analyze-issue-thread \
  --issue-url https://github.com/OpenAgentsInc/openagents/issues/4368 \
  --query "What is the current blocker?" \
  --strategy auto \
  --dry-run
```

Execute the same task under the selected strategy:

```bash
cargo run -p probe-cli -- forge rlm analyze-issue-thread \
  --issue-url https://github.com/OpenAgentsInc/openagents/issues/4368 \
  --query "What is the current blocker?" \
  --strategy rlm \
  --output-dir var/forge-rlm
```

The execution receipt is typed and includes:

- strategy id
- trigger reason
- corpus stats
- iteration count
- sub-LM count
- artifact refs

## TUI

When GitHub issue selection resolves, the chat transcript now records:

- the selected issue handle
- the chosen issue-thread strategy
- the trigger reason
- the current RLM budget envelope when applicable

The footer also shows the selected route badge as `rlm:<strategy>`.

## Tests

The current regression surface covers:

- direct and paper-RLM issue-thread analysis with the same output contract in
  `probe-core`
- typed `heuristic_rlm_trigger_v1` decisions in `probe-decisions`
- CLI dry-run route receipts in `probe-cli`
