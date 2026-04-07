# Stacks patterns

Code patterns and conventions. Patterns are architectural decisions
that shape how features are built.

## Patterns

### Module boundaries

Logic lives in six modules, chosen by concern:

- **main.rs** — CLI definition (clap), command dispatch, output
  rendering. Owns all `println!` calls. Never touches the database or
  parses markdown directly.
- **store.rs** — SQLite data access. Every query lives here. Exposes
  typed methods (`all_chunks`, `search_similar`, `children_of`); callers
  never write raw SQL.
- **index.rs** — Indexing pipeline: scan source files, detect staleness
  via content hashing, parse markdown into sections, insert chunks +
  embeddings inside a transaction.
- **format.rs** — Pure output formatting. TTY detection at startup,
  ANSI wrappers (`dim`, `bold`), tree characters, breadcrumbs, box-top
  banners. No data access, no side effects.
- **embed.rs** — Embedding abstraction. Model loading, single-text and
  weighted heading+content embedding, L2 normalization.
- **error.rs** — Single `Error` enum with `thiserror`. All public
  functions return `Result<T, Error>`.

Dependencies flow one way: `main` → `store`, `index`, `format`, `embed`;
`index` → `store`, `embed`. No cycles.

### Command handlers

Each CLI command maps to a `cmd_*` function in `main.rs`. The pattern:
accept `&Store` for data, typed args from clap, and `&Formatter` for
output. Return `Result<(), Error>`. When a flag changes the output shape
significantly (e.g., `--detail`), delegate to a separate `cmd_*_detail`
function rather than branching inline.

### Output grouping

All output that shows multiple chunks follows the same structure:

1. Group results by source file (adjacent grouping, not sorting)
2. Print the source path as a header (bold in listing mode, box-top
   banner in detail mode)
3. Render items within each group (tree chars in listing, section
   rules in detail)
4. Separate groups with a blank line

This pattern repeats across `cmd_search`, `cmd_search_detail`,
`cmd_index`, and `cmd_index_grep`. It's not yet extracted into a
shared abstraction — the output details differ enough between commands
that the repetition is tolerable, but watch for drift.

### Tree rendering

`render_tree` uses a `depth_prefixes: &mut Vec<bool>` stack to track
whether each ancestor level has more siblings. On each recursive call,
push whether the current node has more siblings (`!is_last`); pop on
return. This produces correct continuation lines (`│` vs space) at
arbitrary depth.

### Auto-reindexing

Every command ensures the index is fresh before executing. The sequence
in `run()`:

1. `scan_sources` — walk `**/*.md` respecting `.gitignore`, compute
   SHA-256 content hashes
2. `has_stale_files` — compare on-disk hashes against indexed hashes
3. `ensure_fresh` — delete removed files, re-index changed files

The embedder is only loaded when needed (search always needs it;
other commands only if stale files exist). This keeps `stacks index`
instant when nothing has changed.

### Indexing transaction

Each file is indexed atomically in a single SQLite transaction:

1. Upsert source record (path + content hash)
2. Delete existing chunks for that source
3. Parse markdown into sections
4. For each section: insert chunk, generate embedding, insert vector
5. Commit

The parent stack algorithm builds the heading hierarchy: retain only
stack entries with level < current section's level, then push the new
chunk. `find_parent` walks the stack in reverse to find the nearest
ancestor.

### Slug generation

Slugs are deterministic: SHA-256 of `(source_path, heading, parent_id)`,
encoded as base36. Start at 4 characters, extend to 8 on collision,
then append a counter. Shorter slugs are preferred for human usability
since they appear throughout the output.

### Search tuning

- Embedding vectors are 30% heading / 70% content, L2-normalized.
  This means the heading provides context (so "List Views" matches
  even when the content doesn't repeat the term) but content dominates.
- `MAX_DISTANCE = 1.15` on L2-normalized vectors (range 0-2) filters
  results that are too dissimilar. `--all` bypasses this threshold.
- Path scoping uses over-fetch + post-filter: when `--path` is set,
  fetch 5x the requested limit from the KNN query, filter by
  `source_path.starts_with(path)`, then take the first N. This
  compensates for the KNN virtual table not supporting arbitrary WHERE
  clauses on joined columns.

### Error handling

Fail-fast via `?` with `thiserror`. All public functions return
`Result<T, Error>`. `main()` catches at the top and prints the error
to stderr. No recovery, no retries, no error context chains — the
codebase is small enough that the error message plus the call site
is sufficient.
