# Plugin configuration and runtime ownership

With its transport features enabled, `UpLinuxStreamerPlugin` implements Zenoh's
plugin factory. Startup reads the named plugin configuration from the supplied
Zenoh runtime, parses it and starts the bridge task on a dedicated Tokio runtime.
The running plugin instance retains the runtime and the task's stop flag.

The bridge composes classic transport endpoints and subscription routing through
`UStreamer`. Plugin loading also requires a compatible Zenoh router/plugin ABI and
the binding-owned native libraries. Use the dependency versions and toolchain of
the built plugin; a library build alone does not test loading it into an arbitrary
router installation. See the binding documentation for native installation details.
