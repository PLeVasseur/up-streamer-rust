# uStreamer example apps

## Publish / subscribe example

### Run the uStreamer

```bash
cargo run --bin ustreamer 
```

### Run the example client app

An example of a client app that interacts with `uStreamer`

```bash
cargo run --example uapp_receive_timer
```

### In another terminal run the publish app

This is using a stand-in for the sommR client library to let us exercise the client app and `uSteamer`

```bash
cargo run --example mdevice_send_timer
```

## RPC server / RPC client example

### Run the uStreamer

```bash
cargo run --bin ustreamer 
```

### Run the example RPC server app

An example of a sommR RPC server app that interacts through `uStreamer`

```bash
cargo run --example mdevice_hello_world_server
```

### In another terminal run the RPC client app

An example of an RPC client app that interacts through `uStreamer`

```bash
cargo run --example uapp_hello_world_rpc_client
```
