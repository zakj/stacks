# Stacks

Semantic search for project documentation. Index your markdown docs,
search by intent, get the chunks that matter.

## Build

Requires Rust (stable). SQLite and the embedding model are bundled —
no external dependencies.

```
cargo build --release
```

The binary is at `target/release/stacks`. On first run, the embedding
model (~30MB) is downloaded and cached locally.

## Contributing

Run `mise run check` before committing. This runs formatting, linting,
and tests in parallel.

## Usage

Run any command from anywhere in your project. Stacks finds the repo
root, indexes all `**/*.md` files (respecting `.gitignore`), and
keeps the index fresh automatically.

```
stacks search how do list views work     # semantic search
stacks search "filter pipeline" -d       # with full content
stacks show a3f2                         # read a section by slug
stacks index                             # browse the heading tree
```

### Agent integration

```
stacks prime --install claude
```

Adds a SessionStart hook so Claude Code agents get usage instructions
on every session. See [DESIGN.md](DESIGN.md) for architecture and
[PATTERNS.md](PATTERNS.md) for code conventions.
