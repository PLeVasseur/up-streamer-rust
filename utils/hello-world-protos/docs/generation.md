# Example protobuf generation

The build script generates application messages from the four checked-in files
under `proto/`, using the locked `protoc-bin-vendored` tool and protobuf-codegen.
It performs no HTTP downloads. `proto/SOURCES.json` records the original source
locations, source hashes and checked-in hashes; those URLs are provenance, not build
inputs fetched at compile time.

The snapshot contains the Hello World service/topics, Google's TimeOfDay and the
imported uProtocol options. The options correspond to the SDK specification input;
its CRLF/trailing-comment whitespace is normalized to repository text conventions.
The topics file gains a final newline. Schema declarations and the generated
descriptor set are unchanged by these formatting differences.

Generated modules are included from Cargo's output directory. Update the checked-in
schema snapshot and provenance together when intentionally changing application
messages, rather than editing generated cache files. These messages are example-owned;
they do not assign payload identities or configure Streamer routes by themselves.
