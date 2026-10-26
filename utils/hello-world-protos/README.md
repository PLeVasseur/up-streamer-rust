# Hello World protobuf examples

This crate supplies generated application messages for the Streamer Hello World
examples. Its build uses the checked-in schema snapshot and vendored `protoc`, with
no build-time HTTP downloads. See [generation inputs](docs/generation.md) for source ownership.
Handwritten entry points use `missing_docs = "deny"`; generated declarations retain
the protobuf generator's own attributes and comments.
