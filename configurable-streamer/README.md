# configurable-streamer

This is a standalone implementation of a uStreamer.
It is implemented to dynamically link between any number of uEntities that use a mix of either Zenoh or MQTT5.

The configurable streamer routes native `UOwnedFrame` values by default. Owned transports are used directly; true zero-copy transports are connected through an owned-frame adapter that copies zero-copy receive leases into owned frames and copies owned egress payloads into transmit loans. Payload codecs remain an application concern: the streamer preserves `PayloadEncoding` and payload bytes but does not deserialize or reinterpret them.

When built with `--features experimental-loaned-frame`, routes between zero-copy endpoints can opt into `routing_mode: "copy_minimized"`. That mode keeps the ingress receive lease out of the owned-frame route logic and copies ordered payload slices directly into the egress transmit loan. It avoids an intermediate owned payload allocation, but it still copies payload bytes and is not zero-copy-preserving forwarding. Omitted `routing_mode` defaults to `"owned"`.

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

`up_streamer_config.route_queue_policy` is optional and defaults to `"backpressure"`. Endpoint-level `route_queue_policy` overrides the default for that endpoint's outgoing routes. The supported values are `"backpressure"` and `"drop_and_report"`; drop-and-report records full ingress queue drops in `UStreamer::data_plane_health()`.

Endpoint `routing_mode` is optional and defaults to `"owned"`. Use `"copy_minimized"` only when both the source endpoint and every forwarding target are zero-copy endpoints and the binary is built with `experimental-loaned-frame`. `copy_minimized_payload_alignment` defaults to `1` and is passed to the egress transmit-loan request. Stable-container copy-minimized routes fail safe instead of forwarding when this alignment is smaller than the stable-container `align` metadata or when the stable-container metadata is malformed.

The `ZENOH_CONFIG.json5` file is used to set Zenoh configurations. By default, it is only used to set listening endpoints, but can be used with more configurations according to [Zenoh's page on it](https://zenoh.io/docs/manual/configuration/#configuration-files).

The 'static_subscriptions.json' is only needed when you set up a publish-subscribe system and can be ignored for a client-service system.
Make sure that the UURI of each pub-sub entity is present at least as a key in this json file!

The 'vsomeip-config/point_to_point.json' is a configuration file only needed for SOME/IP implementations. The list of "services" must include the UEntity IDs of all entities running on the host-protocol (in the reference implementations that means all components running with the Zenoh transport)! The term service in this context comes from SOME/IP and should not be confused with UService entity.

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
- Implement your own custom or proprietary native `UOwnedTransport` or `UZeroCopyTransport` and connect it to a configured endpoint
- Try out a system with your own UEntities. Between MQTT and Zenoh its also possible to send notification type messages
