# Status Log Feed + Realtime Event Surfacing

## Problem

The orchestrator logs lifecycle events (connection, session start, errors) via `tracing`, which only reaches the terminal. The Dioxus UI has no visibility into what's happening with the WebSocket connection. The Debug card shows only the last injection result.

## Design

### New data types (`src/transcription/mod.rs`)

Replace `TranscriptEvent { text, is_final }` with:

```rust
pub enum TranscriptKind {
    Partial,
    Final,
    SessionStarted(String),  // session_id
    Error(String),           // error message
    Info(String),            // generic status
}

pub struct TranscriptEvent {
    pub text: String,
    pub kind: TranscriptKind,
}
```

### Realtime backend changes (`src/transcription/elevenlabs_realtime.rs`)

The receiver task sends `SessionStarted`, `Error`, and `Info` events through `transcript_tx` instead of only logging them via tracing.

### Status log signal

New shared type in `src/ui/status_log.rs`:

```rust
pub enum LogLevel { Info, Warn, Error }

pub struct StatusEntry {
    pub time: String,      // "14:32:05"
    pub level: LogLevel,
    pub message: String,
}

pub struct StatusLog {
    entries: Vec<StatusEntry>,  // capped at 50
}
```

Signal `status_log: Signal<StatusLog>` created in `App`, passed to orchestrator and Debug card.

### Orchestrator changes (`src/orchestrator.rs`)

Push `StatusEntry` messages at each lifecycle point:
- Connecting / connected / session started
- Recording started / stopped
- Partial / final transcripts
- Injection results
- All errors (connection, transcription, injection, input_error from WS)

Match on `TranscriptKind` instead of `is_final`.

### UI changes

**Debug card** — replace "Last: ..." with a scrollable log of the last 50 entries, color-coded by level.

**Home page** (`src/ui/home.rs`) — remove Mistral entries from Quick Settings backend dropdown.

### API key

Already works. The settings UI saves to keyring as `"elevenlabs_api_key"`. The `ws_test` binary reads from `--api-key` CLI arg. Both use the same ElevenLabs endpoint.

## Files changed

| File | Change |
|------|--------|
| `src/transcription/mod.rs` | Replace `is_final` with `TranscriptKind` enum |
| `src/transcription/elevenlabs_realtime.rs` | Surface session/error events via transcript_tx |
| `src/transcription/elevenlabs_batch.rs` | Update TranscriptEvent construction |
| `src/ui/status_log.rs` | New: StatusEntry, StatusLog, LogLevel |
| `src/ui/mod.rs` | Add `pub mod status_log` |
| `src/ui/app.rs` | Create status_log signal, pass to orchestrator + debug card |
| `src/orchestrator.rs` | Push status entries, match on TranscriptKind |
| `src/ui/settings/debug_card.rs` | Render scrollable status log |
| `src/ui/home.rs` | Remove Mistral from Quick Settings dropdown |
| `assets/styles.css` | Styles for log entries + color coding |
