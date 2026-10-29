# Native agreement mode controls

Generated copy-minimized endpoints supply the shared `PreparedAddress` catalog
before any native route is installed. The source island receives forward traffic;
the sink island transmits it. RPC includes the reverse response address, with TX
ownership reversed. Publication and notification need one forwarding direction;
they do not declare an unused reverse route or a new publisher during traffic.
Ordinary catalogs reserve up to 8192 payload bytes. Representative configuration
replaces those logical extents with the actual sensor and encoded metadata sizes,
retaining its existing physical slot and SHM budgets. Binding constructors retain
physical geometry, service mapping and ownership responsibilities.

Startup has distinct configuration and traffic-ready boundaries. After the bridge
prints `READY streamer_configured`, the controller launches both peers before
waiting for their matching checks. The finite active role waits on a fresh,
controller-owned `source-start` file. The controller releases it only after the
passive listener, active first-send barrier and final bridge readiness are observed.
Each boundary is recorded separately; no application message is used as warm-up.
Configured stabilization gates precede release. Sustained workers instead use
their existing phase-control file, with all three actors ready before warm-up.

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
The two small native example representations have equal 272-byte sizes but distinct
descriptors; size equality never authorizes a typed borrow. Native middleware setup
and backend storage guarantees remain documented by their respective bindings.

Prepared validation fixtures reserve the exact source/sink addresses used by their roles across all transport pairs. Notifications involving SOME/IP use resource `0x8000`; other notification and publication fixtures use `0x8001`. RPC reserves request and reversed response addresses separately. Native bridge verification requires checked projection records in both applicable directions.

Finite owned/prepared sources on iceoryx2, LoLa and Zenoh keep their native endpoint alive after their last send until the controller observes the passive role, then accept the exact completion marker and exit. This uses the shared role completion barrier without extra publications or a fixed keepalive delay. RPC retains its existing correlated exchange lifetime. Send counts, receive deadlines and scenario deadlines are unchanged.

## Native library bundles

The selected routes can use fewer backends than the existing executables contain.
Staging inspects the actual ELF `DT_NEEDED` entries of every project executable,
including additional sustained workers, and of the managed libraries themselves.
vSomeIP configuration and service-discovery plugins accompany its core library.
GNU `readelf` inspects metadata without executing the inspected file. Inspection
errors and missing managed dependencies reject the bundle before traffic starts.
Imported bundles receive the same closure check after storage integrity checks.

`LOLA_BRIDGE_LIB_DIR` selects the LoLa bridge directory;
`VSOMEIP_INSTALL_PATH/lib` selects the vSomeIP libraries. An explicit directory
that is incomplete is an error. Without an explicit install, staging searches the
native build outputs under the selected target/profile. Ambient `LD_LIBRARY_PATH`
does not choose these managed inputs. Row workers continue to use only the
bundle library directory in `LD_LIBRARY_PATH`. Standard host ABI libraries,
including vSomeIP's supported Boost components, remain host prerequisites.
