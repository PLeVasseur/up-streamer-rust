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

Currently blocked on the ability for `RpcServer::register_rpc_listener()`
to give us the source UUri.

That would have to come from `RpcClient::invoke_method()` knowing how to tag
the CE appropriately with a generated Response UUri.

### uApp RPC Server & mDevice RPC Client

Currently blocked on the ability of `RpcClient::invoke_method()` to also
at a minimum return `UAttributes` to allow use to route back to sender
with the correct `UUid`, `ttl`, `priority` and so on.

For now did a hack where those things are set within the uStreamer, but
this is not correct.


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
