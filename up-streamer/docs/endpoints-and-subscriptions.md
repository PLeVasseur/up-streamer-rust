# Endpoints, routes and subscription refresh

An `Endpoint` names a classic transport and its routing authority. Construction
stores those values; route installation checks the endpoint relationship. Native
agreement is attached explicitly with `with_native_profile`, not inferred from an
authority name. Owned-frame and native-loan endpoint APIs declare their own
capabilities. See [native route policy](native_routes.md) for complete-route checks.

`UStreamer` owns installed routes, ingress registrations, egress workers and the
local subscription directory. Keep it alive while routes are in service; use the
route lifecycle methods to install and remove forwarding. Message payload encoding
does not by itself select the physical endpoint API or a decoding strategy.

`refresh_subscriptions` fetches a complete snapshot from the supplied
`USubscription` service and applies it to the local directory. Both fetch and apply
failures are returned and recorded. Construction attempts an initial refresh but
can continue in deferred-refresh mode when it fails.

`SubscriptionSyncHealth` reports the completion time and result of the latest
attempt, the preceding result and the latest success time. `subscription_sync_health`
only clones that record; it does not contact the service. It is neither a listing of
subscriptions nor proof that a later refresh will succeed.
