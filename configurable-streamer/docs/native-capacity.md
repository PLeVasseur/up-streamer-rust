# Explicit native capacity configuration

Each iceoryx2 endpoint may add these optional fields to its existing
`iceoryx2_namespace` object:

```json
{
  "root_path": "/tmp/uprotocol-bus",
  "prefix": "sensor_bus_",
  "subscriber_buffer_size": 2,
  "max_borrowed_samples": 2,
  "publisher_initial_max_slice_len": 8388608,
  "publisher_allocation_strategy": "static",
  "max_subscribers": 2,
  "publisher_max_loaned_samples": 1
}
```

Allocation choices are `static`, `best_fit` and `power_of_two`. Omission preserves
the binding defaults. Any of the four new capacity overrides enables the binding's
exact-service checks; all participants must configure matching service capacities.
Zero or overflowing values fail with typed errors. Native construction uses the
fallible binding API. The namespace is retained while applying limits.

The six `iceoryx2_*` role binaries expose corresponding optional flags:
`--iceoryx2-publisher-initial-max-slice-len`,
`--iceoryx2-publisher-allocation-strategy`, `--iceoryx2-max-subscribers` and
`--iceoryx2-publisher-max-loaned-samples`. Their explicit namespace comes from
`UP_ICEORYX2_ROOT_PATH`/`UP_ICEORYX2_PREFIX`. Existing publisher receive/readiness
options and endpoint buffer/borrow names remain unchanged. The controls apply to
both owned and native iceoryx2 endpoints; selecting another backend with these
options is rejected.

Multi-subscriber fan-out remains supported: one is a topology-specific choice,
not a global ceiling. The existing middleware default remains eight. For fixed
large-payload deployments, compute the full encoded sample from actual metadata,
alignment slack and payload before setting its static capacity. Use the existing
`zenoh_shm_segment_size` for each Zenoh provider. Memory forecasts are per service
and process; application queues, transient receive samples and live TX loans are
distinct budgets. Native allocation failure cannot be relabelled as an allowed
application drop. See the [iceoryx2 binding capacity guide](https://github.com/PLeVasseur/up-transport-iceoryx2-rust/blob/main/docs/capacity.md)
for physical service compatibility and allocator ownership.
