# Stacks

A semantic search tool for project documentation. Index your docs,
search by intent, get the chunks that matter.

## Problems

These are ordered by how often they cause real harm, not by how
interesting they are to solve.

### 1. Agent doesn't find relevant guidance

The core problem. An agent is implementing a list view. A "List Views &
Filters" section exists in a doc 4 directories away. Nothing connects
the task to the doc. The agent writes code that works but violates the
project's established pattern. The guidance existed — the agent just
didn't know to look.

This happens because doc delivery is positional (CLAUDE.md loads by
directory) rather than semantic (loaded by relevance to the task). An
agent working in `views/` gets the directory-level doc automatically,
but doesn't get the patterns or access-control docs unless something
tells it to look. There's no query interface — just "read the file or
don't."

### 2. Agent gets too much guidance

A 449-line patterns file encodes 18 conventions; ~30% is universally
relevant, ~70% is task-specific. An agent implementing a list view loads
50 lines about atomic saves it doesn't need. 18 sections compete for
attention, most irrelevant. The agent hedges, over-applies conventions,
or gets confused about which pattern applies. Signal drowns in noise.

### 3. Docs are invisible without manual wiring

Agent guidance scatters across 15+ files: CLAUDE.md, AGENTS.md, rules
directories, skill directories, inline docs, style guides, friction
logs. A new doc only becomes discoverable when someone manually adds it
to an index or CLAUDE.md. Until then it's invisible to every agent.

### 4. Docs go stale

Docs describe patterns that evolve with the code. `patterns.md` says
"use FilterSet" but the code moved to a different approach 3 months ago.
The agent follows the stale doc and writes incorrect code — worse than
no doc at all. Nothing tells you when a doc contradicts what the code
actually does.

### 5. Agent ignores guidance it has

Even with CLAUDE.md loaded, agents sometimes don't follow it. This is
partly a behavioral/prompting problem that search alone doesn't fix. But
a small, targeted chunk ("here's the specific pattern for your task")
may be more effective than a wall of text — focused delivery might
improve compliance even if the core problem is elsewhere.

### What a search tool solves (and doesn't)

Problems 1-3 are clearly search problems: match a task to the right doc
chunk without manual wiring and without loading everything.

Problem 4 (staleness) is a content quality problem. A search tool could
make it *worse* by confidently serving stale guidance. But it can help
surface staleness — if an indexed chunk's source file has changed since
indexing, flag it.

Problem 5 (agent compliance) is a delivery format problem. Search helps
indirectly: a focused, relevant chunk is more likely to be followed than
a wall of text. But the fundamental issue is agent behavior, not
tooling.

## Architecture

### Language: Rust

Speed, single binary distribution, no runtime dependencies. The ecosystem
has mature tooling for every component we need.

### Embeddings: model2vec

model2vec distills a sentence transformer into a static lookup table.
Inference is tokenize → table lookup → pool — no matrix math, no neural
net. The Rust crate (`model2vec`) has no ONNX Runtime dependency.

Model: potion-base-8M (~30MB, 256-dim). Downloaded on first use,
cached locally. Embeds ~8,000 chunks/sec.

No server process. No external service dependency.

### Storage: SQLite

A single `.stacks/index.db` file per repo. Stores:

- Chunk text and metadata (source file, heading, tree structure)
- Embedding vectors via sqlite-vec virtual table
- Content hashes per source file for freshness tracking

Not checked into git — cheap to recreate, expensive to store/fetch.

SQLite gives us queryable metadata for free (`stacks index` is just a
SELECT) and a format that outlives the tool.

### Search: sqlite-vec

sqlite-vec adds KNN search to SQLite via a virtual table. Search is a
single SQL query with MATCH syntax. Path scoping uses over-fetch +
post-filter (the KNN virtual table doesn't support arbitrary WHERE
clauses on joined columns). Bundles statically via the `cc` crate —
negligible binary size impact.

At our expected scale (500-20,000 chunks, 256-dim vectors), sqlite-vec
handles this comfortably. If the corpus grows beyond tens of thousands,
sqlite-vec's approximate indexing kicks in without architectural changes.

### Freshness: content hashing

Store a SHA-256 hash of each source file at index time. On every
command invocation, check hashes of indexed files against current
files. If any changed, re-embed the changed chunks before proceeding.
At our scale, re-indexing a changed file takes milliseconds — cheap
enough to do inline.

This means every command auto-reindexes stale files. No explicit index
command needed.

An `INDEX_VERSION` constant in `src/index.rs` triggers a full re-index
when bumped. Bump it whenever parsing or indexing logic changes (the
content hashes won't catch these since the source files haven't changed).

### Chunking

Split markdown by headings (any level). Each chunk = one heading's
**direct content only** (not including sub-heading content), stored
verbatim. Chunks form a tree mirroring the heading hierarchy.

Content before the first heading in a file becomes a level-0 chunk
using the file stem as its heading (e.g., `README` for `README.md`).
This chunk parents the file's top-level headings, creating a
`file > h1 > h2` containment tree. This means `stacks show` and
ancestor paths naturally include the file context:
`DESIGN > Architecture > Storage` rather than orphaned headings.

Heading embeddings are weighted at 30% of the chunk vector (content
at 70%), L2-normalized, so search matches both the heading context
and body content.

### Schema

```sql
CREATE TABLE sources (
    path TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL
);

CREATE TABLE chunks (
    id INTEGER PRIMARY KEY,
    source_path TEXT NOT NULL REFERENCES sources(path),
    heading TEXT NOT NULL,
    slug TEXT NOT NULL,          -- short unique id (4+ chars)
    parent_id INTEGER REFERENCES chunks(id),
    position INTEGER NOT NULL,  -- document order within file
    content TEXT NOT NULL
);

CREATE VIRTUAL TABLE chunks_vec USING vec0(
    chunk_id INTEGER PRIMARY KEY,
    embedding float[256]
);
```

- **Chunk IDs**: Each chunk gets a short, opaque slug (4 base36 chars,
  extended on collision). Derived from hash of source path + heading +
  parent. Referenced as `source_path#slug` (e.g.,
  `docs/patterns.md#a3f2`). Stored for fast lookup.
- **Tree structure**: `parent_id` links to parent heading (or NULL
  for file-level root chunks). Walk up for ancestors, query down for
  descendants.
- **Document order**: `position` is global within a file, so
  `ORDER BY position` reconstructs original reading order.
- **Vectors**: sqlite-vec virtual table, joined to chunks by id.

## Interface

CLI-only. No MCP server. Agents call it via bash; humans call it directly.

### Commands

```
stacks search <query...>              # semantic search, top 10 results
stacks search <query...> -n 5         # control result count
stacks search <query...> -d           # include complete chunk content
stacks search <query...> -p docs/     # scope to file or directory
stacks search <query...> -a           # skip relevance threshold
stacks show <slug>                    # chunk content + child headings
stacks show <slug> -d                 # chunk + all descendant content
stacks show <source-path>             # top-level headings in file
stacks index                          # full heading tree, all files
stacks index -p <path>                # tree for one file/directory
stacks index -g <keyword>             # filter headings by keyword
stacks prime                          # emit context primer (usage + index)
stacks prime --install claude         # add SessionStart hook for Claude Code
```

Search accepts multiple words without quoting (`stacks search how do
list views work`).

No config required — indexes all `**/*.md` respecting `.gitignore`.
Every command auto-indexes on first run and incrementally re-indexes
stale files (by content hash) on each invocation. To force a full
rebuild, delete `.stacks/index.db`.

**Command roles:**

| Command | Purpose | Analogy |
|---------|---------|---------|
| `search` | Find by meaning | Library catalog search |
| `show` | Read a specific section | Open to a page |
| `index` | Browse the tree | Table of contents |
| `prime` | Agent onboarding | Orientation tour |

### Output

Human-readable by default. Color and formatting when connected to a
TTY; plain text when piped. No `--json` — agents parse prose better
than JSON, and the structured format wasn't useful in practice.

**Design principles:**
- Heading-first layout: lead with human context (ancestor path >
  heading), slug de-emphasized at end (dim on TTY).
- No relevance scores in output — results are ranked, the order is
  the signal.
- Semantic search has no "matching span" to highlight, so default
  search output shows headings only. Use `--detail` or `show` for
  content.

**search:**
```
$ stacks search "how do list views work"

docs/patterns.md
├─ Patterns > List Views & Filters [a3f2]
└─ Conventions > Frontend [b7e1]

docs/access-control.md
└─ Access Control > Role Checks [d3f4]
```

- Results grouped by source file, file header in bold
- Tree chars show grouping within a file
- Ancestor path > heading, slug at end (dim on TTY)

**search --detail:**
```
$ stacks search "how do list views work" -d

── docs/patterns.md ────────────────────────────────────────────────────
Patterns > List Views & Filters [a3f2]

List views use a filter pipeline that chains FilterSet → QuerySet.
Each filter declares its parameter name and lookup type.

Frontend > Creating List Components [b7e1]

To create a list view, use the BaseListView mixin and configure
columns via the schema property.
```

Complete chunk content. Box-top banner per source file. Agents should
use `--detail` to get answers in one shot without follow-up `show`
calls.

**index:**
```
$ stacks index

docs/patterns.md
└─ Patterns [c4d1]
   ├─ Mutations & Transactions [e2a0]
   │  └─ Error Handling [f1b3]
   └─ List Views & Filters [a3f2]
      ├─ Filter Pipeline [d5c7]
      └─ URL Parameter Mapping [b8e4]
```

Tree characters show heading hierarchy. Slugs at end, dim on TTY.

**show:**
```
$ stacks show a3f2

── docs/patterns.md ────────────────────────────────────────────────────
Patterns [c4d1] > List Views & Filters

List views use a filter pipeline that chains FilterSet → QuerySet.
Each filter declares its parameter name and lookup type.

· Filter Pipeline [d5c7]
· URL Parameter Mapping [b8e4]
· Custom Filter Classes [c9a1]
```

With `--detail`, child content is inlined in document order.

### Agent integration

`stacks prime --install claude` adds a SessionStart hook to
`.claude/settings.local.json`. On each session start, the agent
receives a compact context primer with usage instructions and a
live topic list. No CLAUDE.md changes needed.

The install command also adds a permission allowlist entry so the
agent can run stacks without manual approval.

## Cross-repo

### Per-repo (primary)

Index lives in the repo: `.stacks/` directory containing the index file
and config. Searches are scoped to that project. This is the default and
where dogfooding starts.

```
my-project/
  .stacks/
    config.toml    # optional: excludes, overrides
    index.db       # SQLite: chunks + vectors + metadata (gitignored)
  docs/
    patterns.md
    architecture.md
```

### Cross-repo (secondary)

A personal index at `~/.config/stacks/` that spans multiple repos. Each
chunk is tagged with its source repo. Search queries multiple indexes and
merges results by score.

```
stacks search "auth middleware"              # current repo only
stacks search "auth middleware" --global     # all indexed repos
```

Implementation: each repo's index is self-contained. Global search reads
all known indexes, queries each, merges and re-ranks. No shared corpus,
no sync issues.

## Scale

Expected range: 500-2,000 chunks (single repo) to 5,000-20,000 chunks
(cross-repo). A good search tool incentivizes writing more docs, so
plan for the upper end.

- **Embedding**: model2vec does ~8,000 samples/sec. 20,000 chunks
  embeds in ~2.5s. Full re-index is tolerable; incremental re-index
  (only changed files, via content hashing) is near-instant.
- **Search**: 20,000 vectors × 256 dims ≈ 20MB. sqlite-vec handles
  this comfortably.
- **Storage**: SQLite db would be ~50-100MB at the upper end.

## Non-goals

- Replacing CLAUDE.md for always-loaded context
- Indexing source code (this is for docs, not code search)
- LLM-powered summarization or reranking
- Multi-user or server-based deployment
- GUI
