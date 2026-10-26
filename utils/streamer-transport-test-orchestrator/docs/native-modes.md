# Native agreement mode controls

Ordinary connectivity runs generate a Table agreement from the actual linked
application inventory. `--native-single-type <exact-type-name>` instead selects
one exact representation at revision zero through the same exporter and immutable
bundle. It uses the SDK's provisional SingleType artifact and implicit payload
identity; it is not a topic/operation/type map or a normative registry assignment.
The exporter records its actual command, target descriptors and regeneration
inputs. Existing serialized codec selections are independent of this native mode.

SingleType routes containing any classic hop reject unless the generated route
configuration explicitly acknowledges the lost carried identity through
`--allow-single-type-over-classic-hops`. Whole-route validation includes remote
members, not just the local bridge boundary. Native agreement flags currently
apply to ordinary connectivity, not sustained or sharded execution.

`--single-type-controls --artifacts-root <directory>` runs five cases through
the ordinary real-process matrix engine:

- Native-loan positive traffic through iceoryx2 and Zenoh.
- Classic positive traffic with explicit route opt-in.
- Classic rejection without opt-in.
- A classic remote source rejected even though the local bridge boundary uses frames.
- An equal-size but different exact native representation rejected before typed admission.

Positive cases require actual received payload and native identity evidence, with
no retries. Negative cases require the named typed/policy rejection and no received
or forwarded payload; preflight rejection is reported as such. Each case records
arguments, logs, source/dependency/driver identity and complete owned-process cleanup.
Control cases have a separate denominator from connectivity and direct-pair rows.
The two small native example representations have equal272-byte sizes but distinct
descriptors; size equality never authorizes a typed borrow. Native middleware setup
and backend storage guarantees remain documented by their respective bindings.
