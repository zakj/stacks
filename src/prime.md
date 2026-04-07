# Documentation Search with stacks

This project uses `stacks` for semantic documentation search. Use it to find relevant patterns, architecture decisions, and conventions before writing code.

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

When making code changes, check if related documentation needs updating:

- Search for sections related to what you're changing
- Update docs that describe patterns you've modified
- Add documentation for new patterns or conventions
