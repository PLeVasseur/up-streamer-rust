# Package dependency boundaries

Git dependencies carry both an exact source revision and a matching version
requirement. Cargo uses the revision in the source workspace and retains the
version when normalizing an archive. Local package checks must resolve that
normalized dependency to the intended source, rather than silently selecting an
unrelated registry package with a matching name.

The optional `flow-evidence` feature uses the workspace-internal
`streamer-flow-evidence` crate, which remains marked `publish = false`. Integration
tests also use the matching `integration-test-utils` and
`usubscription-static-file` sources. An offline extracted-package check supplies
those exact helper sources and the selected SDK/binding dependencies through its
own verification workspace. These local checks do not imply that the helper or
snapshot dependency versions are available in a public registry.
