# uStreamer example apps

## Publish Examples

### Publish mDevice -> uApp example

#### Run the uStreamer

```bash
cargo run --bin ustreamer 
```

#### In another terminal run the example uApp subscriber

```bash
cargo run --example uapp_receive_timer
```

#### In another terminal run the example mDevice publisher

```bash
cargo run --example mdevice_send_timer
```

### Publish uApp -> mDevice example

#### Run the uStreamer

```bash
cargo run --bin ustreamer 
```

#### In another terminal run the example mDevice subscriber

```bash
cargo run --example mdevice_receive_timer
```

#### In another terminal run the example uApp publisher

```bash
cargo run --example uapp_send_timer
```

## RPC Examples

### mDevice RPC Server & uApp RPC Client

#### Run the uStreamer

```bash
cargo run --bin ustreamer 
```

#### In another terminal run the example mDevice RPC server

```bash
cargo run --example mdevice_hello_world_server
```

#### In another terminal run the example uApp RPC client app

```bash
cargo run --example uapp_hello_world_rpc_client
```
### uApp RPC Server & mDevice RPC Client

#### Run the uStreamer

```bash
cargo run --bin ustreamer 
```

#### In another terminal run the example uApp RPC server

```bash
cargo run --example uapp_hello_world_server
```

#### In another terminal run the example mDevice RPC client app

```bash
cargo run --example mdevice_hello_world_rpc_client
```
