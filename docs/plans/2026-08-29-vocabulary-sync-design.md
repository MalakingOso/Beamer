# Vocabulary sync

`vocabulary.txt` follows the notes corpus to the other machine. Today it sits
alongside `config.toml` as machine-local, and a term added on callisto has to
be typed again on bearcave.

## What carries it

The existing automerge document, the one `sync_server` and `sync_client`
already move between machines. No new transport, no second protocol, no
Syncthing folder.

## Why a scalar at `ROOT`, not a map

The whole list lives at `ROOT["vocabulary"]` as one newline-joined string,
byte-identical to what `vocabulary.txt` already holds.

A map keyed by term is the shape `doc_notes` and `doc_tasks` use, and it would
merge concurrent additions instead of losing one side's. It was rejected
anyway, for two reasons that are specific to this list rather than general:

- **Order is load-bearing.** `keyterms.rs` sends the first 100 terms (50 for
  realtime) and drops the rest, so list order decides which terms reach the
  API at all. An automerge map has no order, so a map would need a position
  field per term, and two machines appending while disconnected would both
  claim the same index.
- **`rename` is in place, deliberately.** Under a term-keyed map the key *is*
  the term, so a rename becomes remove-then-add — exactly the pattern
  `Vocabulary::rename`'s doc comment says not to reimplement, because it
  appends and the on-disk order stops matching what the Vocab page shows.

A scalar has neither problem: order is just the string, and a rename is an
ordinary edit to it. `ROOT` is also the one object id every automerge document
shares, so a scalar there needs no entry in `genesis.automerge` and cannot hit
the two-objects-at-one-key conflict that genesis exists to prevent.

The cost is real and worth stating: **concurrent edits do not merge.** Two
machines that both edit while disconnected keep one side's list and discard
the other's. That is acceptable here because the list is small, rarely edited,
and never edited on both machines at once in practice — none of which is true
of notes, which is why notes get a CRDT and this does not.

## The three-way comparison

`Vocabulary` is not a Dioxus signal. Every call site does a fresh
`Vocabulary::load()` from disk, mutates, and saves; there is no in-memory copy
for a reconcile to diff against, the way `flush_stores` diffs `NoteStore`.

Rather than plumb a `SyncDoc` handle into `Vocabulary::save()` — which would
make `config::` depend on `notes::` and touch all four mutation paths — the
whole feature lives in the document pass, comparing three values:

| file vs `last` | doc vs `last` | Action |
|---|---|---|
| same | same | nothing; no lock taken |
| changed | same | local edit — push the file into the document |
| same | changed | remote edit — write the document to `vocabulary.txt` |
| changed | changed | collision — the document wins |

`last` is the content both sides agreed on at the end of the previous pass,
held in memory on `SyncDoc`. On the first pass of a run it is `None`, which
reads as "no baseline yet": the document wins if it has anything, otherwise
the file seeds it.

**The document wins a collision.** Both machines apply the same rule, so they
converge on the next pass instead of ping-ponging, and it matches how the rest
of the corpus already behaves — the document is the source of truth, the file
is a mirror. The lost side is a handful of re-typed terms, not a note.

## Where the state lives

On `SyncDoc`, not `NoteStore`:

```rust
vocab_path: Option<PathBuf>,   // None = vocabulary sync off
last_vocab: Option<String>,    // the baseline above
```

`None` is the default, so `sync_server` (which has no vocabulary and no
business writing one) and every existing test are unaffected without being
touched. `NoteStore`'s own construction opts in through a builder, and it is
the only caller that does. `NoteStore` gains no field, which matters because
it is built by struct literal in six test files that would otherwise all need
editing.

`Vocabulary` itself is not modified at all: no new dependency, no changed call
sites, its whole test suite still valid.

## Gating

The same switch as everything else: `config.sync.url` empty means off, the
precedent `note_hotkey` set and `sync_client::should_start` already reads. One
sync switch for the machine, not one per data type.

## Ordering inside the pass

Vocabulary is handled *after* the notes/tasks merge, for the same reason
`flush.rs` documents for its own ordering: the merge is what brings the other
machine's document content in, so reading `ROOT["vocabulary"]` before it would
compare against a stale value and mistake an incoming change for no change.

A vocabulary push marks the document dirty through the existing
`mark_pending_save`, so it rides the same 500ms save the notes corpus does
rather than adding a second write path.

## Testing

The decision table is a pure function over three `Option<&str>` values, tested
without touching disk. The I/O wrapper is tested against a PID-scoped temp
path, the convention `Vocabulary`'s own tests and `lifecycle`'s `temp_store`
already use, so no test ever reads or writes the real vocabulary.

## Deployment

**Nothing needs deleting.** Clearing the existing documents was on the table
while a third root map was, because that would have meant regenerating
`genesis.automerge` and old documents would no longer merge with new ones. The
scalar removed that requirement: `ROOT` is already shared by every document
ever created, so a document written before this feature simply has no
`vocabulary` key, reads as `None`, and gets seeded from the local file on its
first pass.

That matters more than a spared step — bearcave already holds real notes in
its document. Deleting it would have destroyed them for a feature that never
needed it.
