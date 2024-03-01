# uStreamer

An implementation of a uStreamer component based on the uProtocol
specification [available here](https://github.com/eclipse-uprotocol/up-spec/tree/main/up-l2/dispatchers).

## Examples

Check the `examples/` folder for sets of examples which test the current
functionality.

## Implementation Status

### Alpha status

Currently uses a number of feature branches to pull together the necessary
features. Needs to get these changes upstreamed into `up-rust` and `up-client-zenoh-rust`
and then depend on the upstream.

### Checklist

Basically, none of the below yet, only the the basics.

* **MUST** support At-least-once delivery policy, this means that the dispatcher will make every attempt to dispatch the CE to the intended Receiver
   * **MUST** queue CEs not successfully acknowledged (transport level at-least-once delivery confirmation described above)
   * **MUST** attempt to retry transmission of the CE. Retry policy is specific to the dispatcher implementation 
   * Dispatcher **MUST NOT** discard CEs unless either CE has expired (CE.ttl), or the egress queue is full. CEs that cannot be delivered are sent to a Dead Letter Office Topic
* **MAY** support additional CE delivery policies in general or per topic in the future
* **SHOULD** dispatch in order that it received the CE
* **MAY** batch CEs when delivering to the Receiver
* CEs that cannot be delivered **MUST** be sent to the Dead Letter topic (DLT)
   * DLT **MUST** include at least the CE header, SHOULD contain the full CE 
   * DLT **MUST** include the reason for the failed delivery attempt using error codes defined in google.rpc.Code 
   * uEs **MUST** be able to subscribe to the DLT to be notified of message deliver failures
If the uP-L1 delivery method is push:
* **SHALL** provide an API to start/stop dispatching of CEs per-topic, this is to avoid having to queue CEs on the Receiver if the Receiver is not ready to receive the CEs