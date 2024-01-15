# uStreamer example apps

## Simple publish / subscribe example

### Run the uStreamer

```bash
cargo run --bin ustreamer 
```

### Run the example client app

An example of a client app that interacts with `uStreamer`

```bash
cargo run --example receive_hello_world
```

### In another terminal run the publish app

This is a stand-in dummy to let us exercise the client app and `uSteamer`

```bash
cargo run --example send_hello_world
```

## Simple RPC server / RPC client example

### Run the uStreamer

```bash
cargo run --bin ustreamer 
```

### Run the example RPC server app

An example of an RPC server app that interacts through `uStreamer`

```bash
cargo run --example hello_world_rpc_server
```

### In another terminal run the RPC client app

An example of an RPC client app that interacts through `uStreamer`

```bash
cargo run --example hello_world_rpc_client
```
