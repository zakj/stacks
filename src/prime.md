# Documentation Search with stacks

This project uses `stacks` for semantic documentation search. Use it to find relevant patterns, architecture decisions, and conventions.

## When to use stacks

- **Grep**: you know the exact string or symbol (`filter_set`, `6001`, `handleAuth`)
- **stacks search**: you know the concept but not the vocabulary or location ("how do we handle auth?", "what's the proxy setup?")
- Use both together: stacks for discovery, then grep for exhaustive coverage
- Run stacks queries in the main conversation, not in sub-agents — they have Bash access but don't know stacks exists

## Workflow

1. `stacks search <query> -d` — find docs by meaning, with full content
2. `stacks show <slug>` — read a specific section (slugs appear as `[abc1]` in output)
3. `stacks index` — browse the full heading tree

## Key Commands

- `stacks search <query>` — semantic search; multiple words without quotes
- `stacks search <query> -d` — include full chunk content (recommended)
- `stacks search <query> -p <path>` — scope to a file or directory
- `stacks search <query> -a` — skip relevance threshold, return all results
- `stacks show <slug>` — show a section by slug ID
- `stacks show <slug> -d` — section + all descendant content
- `stacks index` — full heading tree, all files
- `stacks index -p <path>` — tree for one file or directory
- `stacks index -g <keyword>` — filter headings by keyword

## Reading Output

Search and index output show headings with slug IDs in brackets:

    Patterns > Command handlers [tdke]

Pass the slug to `show` to read the full content: `stacks show tdke`

## search vs index -g

- `search` is semantic: finds content by meaning, searches headings + body
- `index -g` is lexical: filters headings by exact keyword match

Use `search` when you know what you need but not where it is. Use `index -g` when you know the vocabulary.

## Maintaining Documentation

After making code changes, check if related documentation needs updating:

- `stacks search` finds docs that describe the behavior you changed (conceptual matches)
- `grep` finds docs that reference specific functions or files you changed (literal matches)
- Update docs that describe patterns you've modified
- Add documentation for new patterns or conventions
