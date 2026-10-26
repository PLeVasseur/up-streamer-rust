# Static subscription file and cache

The file is a JSON object mapping topic URI strings to arrays of subscriber URI
strings. The service normalizes topic resources to0x8001 and deduplicates topic /
subscriber pairs. Invalid URI entries, non-array subscriber lists and non-string
subscribers are logged and skipped; an unreadable file, malformed JSON or non-object
top level fails the fetch. Matching fetches return `Subscribed` entries.

`new` uses `AlwaysReload`; `with_reload_strategy` can select `CacheOnFirstRead`.
Construction stores the path without accessing it. Cached mode reuses the first
successful snapshot until `clear_cached_subscriptions` removes it. Cache-lock
poisoning returns `Internal`. Clearing the cache does not write the source file.

Mutation operations are unsupported by this read-only implementation. Streamer
refresh-health metadata describes snapshot refresh attempts separately from the
subscription contents returned here.
