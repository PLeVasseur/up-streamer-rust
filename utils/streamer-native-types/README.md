# Streamer native application types

This dependency-light crate defines the actual SelectedWireNativePayload and
NativeFlowPayload representations used by the example roles and validation tools.
Their deployment-stable names and fixed layouts remain distinct even though both
have the same total size. Both concrete types opt into SDK target-side inventory.

The companion `streamer-native-inventory` executable links this crate directly.
Deployment tooling assigns IDs to the exported descriptors; payload types do not
own an encoding ID or a runtime allocation table. See the Streamer native-deployment
guide for generation, manifest distribution and admission.
# Optional large sensor representations

`payload-contract-large-fixtures` links and registers the existing SDK
`LidarPointCloudHesaiAt128V1` and `CameraBayerRggb12pFrame8mpV1`, retaining their
actual defining types, full layouts and names. No smaller substitute or wrapper
representation is introduced. See the [worker guide](../../example-streamer-uses/docs/sustained-workloads.md).
