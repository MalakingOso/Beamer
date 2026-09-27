# Beamer Roadmap

## Known issues

- `failure_message` in `src/llm/client.rs` reports an unreachable host as reachable-but-silent: it checks `timed_out` before `connect_failed`, but reqwest sets `is_timeout()` for a connect timeout too. Swap the branches and update the `failures_are_described_in_the_users_terms` test with it.

## Low priority

- `debug_logging` config toggle has no runtime effect — wiring it would control injection trace logging
