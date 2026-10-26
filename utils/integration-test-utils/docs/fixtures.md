# Controlled client and listener fixtures

`UPClientFoo` receives from the supplied broadcast bus on its own thread and
dispatches to matching source/sink registrations through SDK listener admission.
A sinkless filter matches sinkless messages; it is not an all-sinks wildcard.
Its count measures admitted listener deliveries, including multiple registrations.
`UPClientFailingRegister` injects registration failure and is not a working backend.

`run_client` starts a dedicated-thread sender with a single-thread Tokio runtime.
The test controller pauses it, supplies a `ClientCommand`, waits for acknowledgment
when appropriate, and resumes or stops it. To obtain the returned send history,
request pause with `Stop` and join the handle. Receive loops finish when their bus
channels close. Send history and listener stores are growing test vectors; initial
capacity is not a memory bound and these fixtures are not a soak resource oracle.

Count waiters have explicit deadlines and panic with the observed count on timeout.
The discrepancy assertion requires nonzero counts and treats its slack argument as
a fraction of sent messages. The ordering assertion groups by source and message
kind and permits equal UUID timestamps; it does not claim strict sequence ordering.

URI helpers build local/remote service fixtures at major version 1/resource 2.
Message helpers derive event or resource-zero recipient URIs as needed and produce
payloadless examples. Response helpers create fixture correlation UUIDs rather than
replying to a captured request. Use actual request metadata for protocol-level RPC tests.
