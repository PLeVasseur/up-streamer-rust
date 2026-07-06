# configurable-streamer

This is a standalone implementation of a uStreamer.
It is implemented to dynamically link between any number of uEntities that use Zenoh, MQTT5, or the optional zero-copy transports.

## Supported Setups

Here are the setups that can be built with these streamers and different entities with the Zenoh and MQTT transports.
Run the streamer with:

```bash
cargo run -- --config="CONFIG.json5"
```

The setups that include SOME/IP entities are currently not supported by the configurable streamer. To run those please refer to the "example-streamer-implementations" folder and the zenoh_someip streamer binary found there!

### Client-Service Setups

In a setup with one client and one service, the service runs in the background while the client periodically makes requests to it.
Once the server receives a request it will respond with a reply.
The request message contains information on a "sink" (the URI of the service entity which it tries to reach) and a source (the URI of the client so that the service knows where to send the response to).

For a single setup you can choose either:
- a Zenoh Client and an MQTT Service (A cars' software requesting information from the cloud)
- an MQTT Client and a Zenoh Service (A backend service trying to pull telemetry data from a running car)
- a Zenoh Client and a SOME/IP Service (The infotainment system requesting some mechatronics sensor data)
- or a SOME/IP Client and a Zenoh Service (A mechatronics component asking the infotainment system for input)

### Publish-Subscribe Setups

This setup is more straight forward and consists of one publisher broadcasting messages to a topic and a subscriber who listens to the topic.
Messages that the publisher sends contain a "topic" that the message will be available on. There is no response expected so publish messages do not contain the publishers URI.
The subscriber listens to one or multiple topics via a filter.

For this setup you can also choose:
- a Zenoh Publisher and an MQTT Subscriber (A backend service getting live data from a car)
- an MQTT Publisher and a Zenoh Subscriber (A car getting over the air traffic information)
- a Zenoh Publisher and a SOME/IP Subscriber (An autoedge app pushing some configurations to a mechatronics component)
- or a SOME/IP Publisher and a Zenoh Subscriber (An infotainment app getting live data from a sensor)

### Notification setups

There are currently no example entities for notification type messages. These do not exist for SOME/IP but do exist for Zenoh and MQTT. It should be relatively straight forward to implement the yourself if your system needs them!

## Understanding the Configuration Files

Reference the `CONFIG.json5` configuration file to understand the basic configuration options for the Streamer.

The `ZENOH_CONFIG.json5` file is used to set Zenoh configurations. By default, it is only used to set listening endpoints, but can be used with more configurations according to [Zenoh's page on it](https://zenoh.io/docs/manual/configuration/#configuration-files).

The 'static_subscriptions.json' is only needed when you set up a publish-subscribe system and can be ignored for a client-service system.
Make sure that the UURI of each pub-sub entity is present at least as a key in this json file!

The 'vsomeip-config/point_to_point.json' is a configuration file only needed for SOME/IP implementations. The list of "services" must include the UEntity IDs of all entities running on the host-protocol (in the reference implementations that means all components running with the Zenoh transport)! The term service in this context comes from SOME/IP and should not be confused with UService entity.

## Zero-Copy Example Configurations

The configurable streamer can expose copy-minimized routes when it is built with `experimental-copy-minimized-routing` and the matching zero-copy transport features. The example files use the current grouped transport schema under `transports`:

- `CONFIG_ZENOH_ICEORYX2_ZEROCOPY_EXAMPLE.json5` routes between Zenoh shared memory and iceoryx2.
- `CONFIG_LOLA_ZEROCOPY_EXAMPLE.json5` routes between Zenoh shared memory and LoLa using `MW_COM_CONFIG_LOLA.json`.
- `CONFIG_ZEROCOPY_EXAMPLE.json5` includes Zenoh shared memory, iceoryx2, and LoLa with pairwise copy-minimized forwarding.
- `CONFIG_ZEROCOPY_MISMATCH_NEGATIVE_EXAMPLE.json5` documents the expected startup failure for an unsupported wire format declaration.
- `MW_COM_CONFIG_LOLA.json` is the LoLa MW COM service/event fixture used by the LoLa examples.

LoLa-backed examples use checked-in S-CORE MW COM deployment manifests such as
`MW_COM_CONFIG_LOLA.json`. The streamer passes these paths explicitly through
`lola_mw_com_config_file`, so no copy into `./etc/mw_com_config.json` is needed
for these examples. A native LoLa process can initialize S-CORE with only one MW
COM manifest, so every LoLa endpoint in a single configurable-streamer process
must reference the same resolved manifest path. Use one complete manifest that
contains all LoLa services/events needed by that example.

On Linux, S-CORE LoLa writes runtime service-discovery and partial-restart state
under `/tmp/mw_com_lola`. This is LoLa runtime state, not streamer config.
Repeated local runs after crashes may require cleaning that directory, but only
after all LoLa-backed streamer and role processes have stopped.

Each copy-minimized endpoint sets `routing_mode: "copy_minimized"`. Copy-minimized forwarding uses `forwarding_routes` entries instead of the legacy `forwarding` string array so each configured route can declare the selected wire format explicitly:

```json5
forwarding_routes: [
  { endpoint: "iceoryx2-zc", wire_format: "protobuf" },
]
```

The route declaration must use the same wire format for the ingress and egress adapter pair. Unsupported wire format names, missing `wire_format` on copy-minimized routes, MQTT endpoints, owned-only endpoints, or uncompiled zero-copy transports fail during startup before forwarding is registered. The current implementation preserves the one-copy copy-minimized route semantics from `up-streamer`; it is not a generic no-copy forwarding path. If route metadata cannot be decoded for the configured wire format, the selected-wire adapter drops or rejects the frame before Streamer forwarding.

Owned/default routes can continue to use the legacy `forwarding` array and do not require `wire_format`. MQTT endpoints cannot use copy-minimized routing; these examples keep the required MQTT transport section with an empty endpoint list.

Run the examples from the `configurable-streamer` directory so the relative config file paths resolve:

```bash
cargo run -p configurable-streamer --features experimental-copy-minimized-routing,zenoh-zero-copy,iceoryx2-zero-copy -- --config="CONFIG_ZENOH_ICEORYX2_ZEROCOPY_EXAMPLE.json5"
```

```bash
cargo run -p configurable-streamer --features experimental-copy-minimized-routing,zenoh-zero-copy,lola-transport -- --config="CONFIG_LOLA_ZEROCOPY_EXAMPLE.json5"
```

```bash
cargo run -p configurable-streamer --features experimental-copy-minimized-routing,zenoh-zero-copy,iceoryx2-zero-copy,lola-transport -- --config="CONFIG_ZEROCOPY_EXAMPLE.json5"
```

The LoLa feature uses the default bundled native bridge build. If your environment does not provide `bazel`, set `BAZEL` to a Bazel or Bazelisk binary before building. The Streamer smoke matrix can also bootstrap the pinned Bazelisk with `scripts/ensure-lola-bazelisk.sh` and caches it under `.cache/tools/`. Configurations with `mqtt.endpoints: []` do not initialize MQTT or require a broker at startup.

## Running the Streamer in an example service mesh

### Running the uStreamer binary

To run one of the basic examples and see two entities with different transports communicate, you'll need to first run the streamer (see above) to bridge between the two transports in a terminal. This implementation should work out of the box with the given examples and the "CONFIG.json5".

First run an MQTT broker for example by running:

```bash
mosquitto -d
```

Run the streamer with the default configuration file from here (the example-streamer-implementation folder):

```bash
cargo run -- --config="DEFAULT_CONFIG.json5"
```

This starts the streamer which should now be idle. As soon as a client tries to connect with the streamer, the connection will be logged.
The streamer is set to have Zenoh as its "host protocol" or "host transport". This means that the streamer lives in the same component as the Zenoh transport, and shares its authority.
In this setup "authority-b" is the authority of the Zenoh component (in this example the ECU), "authority-a" is the authority of the MQTT component (i.e. the cloud).

### Running the Entities in a zenoh - MQTT5 setup

Execute the following command from the project root directory to start two of the example UEntities:

```bash
cargo run --bin <transport_entity> --features=<check cargo.toml or logs to see which feature flags you need>
```

Depending on the setup you want to test, chose any of these combinations for your two UEntities:

| Entity 1        | Entity 2         |
| --------------- | -------------    |
| mqtt_client     | zenoh_service    |
| mqtt_service    | zenoh_client     |
| mqtt_publisher  | zenoh_subscriber |
| mqtt_subscriber | zenoh_publisher  |

The service and client will run forever. Every second a new request message is sent from the client via zenoh. That Zenoh message is caught and routed over MQTT to the service. The response to the request makes the same journey in reverse.

### Running the Entities in a zenoh - SOME/IP setup

Execute the following command from the project root directory to start one of the Entities:

```bash
cargo run -p example-streamer-uses --bin <transport_entity>
```

Depending on the setup you want to test, chose any of these examples:

| Entity 1        | Entity 2         |
| --------------- | -------------    |
| someip_client     | zenoh_service    |
| someip_service    | zenoh_client     |
| someip_publisher  | zenoh_subscriber |
| someip_subscriber | zenoh_publisher  |

The two entities will run forever and exchange messages between each other.

## Going forward from here

If you have familiarized yourself with the streamer to this point you should be able to continue by yourself.
If the two reference implementations are not enough for your system you can consider the following next steps:

- Create a streamer between SOME/IP and MQTT (mind that SOME/IP cannot act as the host-transport)
- Run a streamer that can forward messages between all three transports
- Implement your own custom or proprietary UTransport and connect it to one of the three officially supported ones
- Try out a system with your own UEntities. Between MQTT and Zenoh its also possible to send notification type messages
