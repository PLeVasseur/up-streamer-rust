# Named benchmark comparison

`GuardrailInput` selects a Criterion output root, baseline and candidate names,
and three percentage thresholds. Every identity in `REQUIRED_BENCHMARK_IDS` must
exist in both sets. The reader requires the measured-value and iteration-count
CSV columns, converts samples to nanoseconds per iteration and compares medians.
Missing or malformed inputs return an error rather than partial acceptance.

Latency uses the percentage increase in median nanoseconds per iteration.
Throughput uses the percentage decrease in derived operations per second.
The allocation-sensitive proxy uses timing with its own threshold; it does not
count allocations or establish an allocation bound. Negative deltas indicate
improvement; a result passes when its regression does not exceed its threshold.

`GuardrailReport` records input identities, thresholds, medians, outcomes and
failure explanations. `write_report` emits JSON and `render_summary_table` emits
a readable table. Environment control and selecting comparable benchmark inputs
remain the caller's responsibility.
