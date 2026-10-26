# Smoke execution and evidence contract

`ScenarioTemplate` describes each classic smoke scenario's build commands,
required paths, process roles, readiness and deadline. The runner also knows native
loan scenarios through a separate internal route model. `scenario_ids()` exposes
the eight classic scenarios; `matrix_scenario_ids()` includes the seven native
scenarios. This finite smoke universe is distinct from the full profile matrix.

`ScenarioCliArgs` controls finite send count/interval, deadlines, artifact root,
claims location and optional branch/environment checks. MQTT can use Docker Compose,
an external real broker or an owned native Mosquitto process. Native-library lookup
uses explicit environment values before inspecting compatible build outputs.

Claims are loaded from a scenario-matching JSON file. A directory override selects
`<scenario-id>.json`; a file override selects exactly that file. Missing/malformed
input, invalid regexes, duplicate claim IDs and invalid threshold combinations fail
loading. Positive claims require at least their configured match count; forbidden
claims require zero matches. Evaluation examines the whole selected log and retains
read/regex failures as failed outcomes. It does not infer whole-message fidelity
from a readiness marker or a minimum log count.

Each `ManagedProcess` has a separate process group and a combined output log.
The caller explicitly waits or shuts it down: SIGINT and SIGTERM use bounded grace
periods, and expiration is an error rather than successful cleanup. Status accessors
describe observations made through that handle; they are not a system-wide leak audit.

Scenario reports retain launch metadata, timings, exact claim outcomes, classification,
failure reasons, reproduction commands and artifact paths. Matrix summaries aggregate
the selected smoke scenarios. JSON and text writers return the paths they wrote;
callers choose fresh artifact directories when earlier evidence must be preserved.
