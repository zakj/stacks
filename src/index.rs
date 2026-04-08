use std::collections::HashMap;
use std::path::Path;

use ignore::WalkBuilder;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use sha2::{Digest, Sha256};

use crate::embed::Embedder;
use crate::error::Error;
use crate::store::{NewChunk, Store};

/// Bump when parsing or indexing logic changes to trigger a full re-index.
pub const INDEX_VERSION: u32 = 1;

struct Section {
    heading: String,
    level: usize, // 0 = file-level (before first heading), 1-6 = h1-h6
    content: String,
}

/// Walk the project for markdown files, returning relative paths and content hashes.
pub fn scan_sources(store: &Store) -> Result<HashMap<String, String>, Error> {
    let project_root = store.project_root();
    let stacks_dir = store.root();
    let mut on_disk = HashMap::new();
    for entry in walk_markdown(project_root).flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        if path.starts_with(stacks_dir) {
            continue;
        }
        let rel = path
            .strip_prefix(project_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let content = std::fs::read_to_string(path)?;
        let hash = sha256_hex(&content);
        on_disk.insert(rel, hash);
    }
    Ok(on_disk)
}

pub fn has_stale_files(store: &Store, on_disk: &HashMap<String, String>) -> Result<bool, Error> {
    if store.needs_embedding()? {
        return Ok(true);
    }
    let indexed = store.all_sources()?;
    if on_disk.len() != indexed.len() {
        return Ok(true);
    }
    for (path, hash) in on_disk {
        match indexed.get(path.as_str()) {
            Some(existing) if existing == hash => {}
            _ => return Ok(true),
        }
    }
    Ok(false)
}

pub fn ensure_fresh(
    store: &Store,
    embedder: &Embedder,
    on_disk: &HashMap<String, String>,
) -> Result<(), Error> {
    if store.needs_embedding()? {
        store.clear_all()?;
    }

    let indexed = store.all_sources()?;

    for path in indexed.keys() {
        if !on_disk.contains_key(path.as_str()) {
            store.delete_source(path)?;
        }
    }

    for (rel_path, hash) in on_disk {
        let needs_index = indexed
            .get(rel_path.as_str())
            .is_none_or(|existing| existing != hash);
        if needs_index {
            let content = std::fs::read_to_string(store.project_root().join(rel_path))?;
            index_file(store, embedder, rel_path, &content, hash)?;
        }
    }

    Ok(())
}

fn walk_markdown(project_root: &Path) -> ignore::Walk {
    WalkBuilder::new(project_root)
        .hidden(false)
        .build()
}

fn index_file(
    store: &Store,
    embedder: &Embedder,
    rel_path: &str,
    content: &str,
    hash: &str,
) -> Result<(), Error> {
    let file_stem = std::path::Path::new(rel_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| rel_path.to_string());
    let sections = parse_markdown(content, &file_stem);
    store.transaction(|store| {
        store.upsert_source(rel_path, hash)?;
        store.delete_chunks_for_source(rel_path)?;

        let mut parent_stack: Vec<(usize, i64)> = Vec::new();

        for (position, section) in sections.iter().enumerate() {
            let parent_id = find_parent(&parent_stack, section.level);
            let slug = generate_slug(store, rel_path, &section.heading, parent_id)?;

            let chunk_id = store.insert_chunk(&NewChunk {
                source_path: rel_path,
                heading: &section.heading,
                slug: &slug,
                parent_id,
                position: position as i32,
                content: &section.content,
            })?;

            let embedding = embedder.embed_chunk(&section.heading, &section.content);
            store.insert_embedding(chunk_id, &embedding)?;

            parent_stack.retain(|(lvl, _)| *lvl < section.level);
            parent_stack.push((section.level, chunk_id));
        }

        Ok(())
    })
}

fn find_parent(parent_stack: &[(usize, i64)], level: usize) -> Option<i64> {
    if level == 0 {
        return None;
    }
    parent_stack
        .iter()
        .rev()
        .find(|(lvl, _)| *lvl < level)
        .map(|(_, id)| *id)
}

fn parse_markdown(content: &str, file_stem: &str) -> Vec<Section> {
    let parser = Parser::new_ext(content, Options::empty());

    // Scan for heading boundaries and extract heading text/level.
    struct HeadingInfo {
        text: String,
        level: usize,
        start: usize,
        end: usize,
    }
    let mut headings: Vec<HeadingInfo> = Vec::new();
    let mut in_heading = false;
    let mut heading_text = String::new();
    let mut heading_level: usize = 0;
    let mut heading_start: usize = 0;

    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                in_heading = true;
                heading_level = level as usize;
                heading_start = range.start;
            }
            Event::End(TagEnd::Heading(_)) => {
                in_heading = false;
                headings.push(HeadingInfo {
                    text: std::mem::take(&mut heading_text),
                    level: heading_level,
                    start: heading_start,
                    end: range.end,
                });
            }
            Event::Text(t) if in_heading => {
                heading_text.push_str(&t);
            }
            Event::Code(t) if in_heading => {
                heading_text.push('`');
                heading_text.push_str(&t);
                heading_text.push('`');
            }
            Event::SoftBreak | Event::HardBreak if in_heading => {
                heading_text.push(' ');
            }
            _ => {}
        }
    }

    // Slice raw content between heading boundaries.
    let mut sections: Vec<Section> = Vec::new();

    let pre_end = headings.first().map_or(content.len(), |h| h.start);
    let pre_content = content[..pre_end].trim();
    if !pre_content.is_empty() {
        sections.push(Section {
            heading: file_stem.to_string(),
            level: 0,
            content: pre_content.to_string(),
        });
    }

    for (i, heading) in headings.iter().enumerate() {
        let content_end = headings.get(i + 1).map_or(content.len(), |next| next.start);
        let body = content[heading.end..content_end].trim();
        sections.push(Section {
            heading: heading.text.clone(),
            level: heading.level,
            content: body.to_string(),
        });
    }

    sections
}

fn generate_slug(
    store: &Store,
    source_path: &str,
    heading: &str,
    parent_id: Option<i64>,
) -> Result<String, Error> {
    let mut hasher = Sha256::new();
    hasher.update(source_path.as_bytes());
    hasher.update(heading.as_bytes());
    hasher.update(parent_id.unwrap_or(-1).to_le_bytes());
    let hash = hasher.finalize();

    let mut num = u64::from_le_bytes(hash[..8].try_into().unwrap());

    // Try lengths 4–8: short enough to type, long enough to avoid collisions.
    for len in 4..=8 {
        let slug = to_base36(num, len);
        if !store.slug_exists(&slug)? {
            return Ok(slug);
        }
        num = num.wrapping_add(u64::from_le_bytes(hash[8..16].try_into().unwrap()));
    }

    for i in 0u64.. {
        let slug = to_base36(num.wrapping_add(i), 8);
        if !store.slug_exists(&slug)? {
            return Ok(slug);
        }
    }
    unreachable!()
}

fn to_base36(mut num: u64, len: usize) -> String {
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut result = Vec::with_capacity(len);
    for _ in 0..len {
        result.push(CHARS[(num % 36) as usize]);
        num /= 36;
    }
    String::from_utf8(result).unwrap()
}

fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_markdown() {
        let md = "# Title\n\nIntro paragraph.\n\n## Section One\n\nContent one.\n\n## Section Two\n\nContent two.\n";
        let sections = parse_markdown(md, "test");
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].heading, "Title");
        assert_eq!(sections[0].level, 1);
        assert_eq!(sections[0].content, "Intro paragraph.");
        assert_eq!(sections[1].heading, "Section One");
        assert_eq!(sections[1].level, 2);
        assert_eq!(sections[1].content, "Content one.");
        assert_eq!(sections[2].heading, "Section Two");
        assert_eq!(sections[2].level, 2);
        assert_eq!(sections[2].content, "Content two.");
    }

    #[test]
    fn parse_nested_headings() {
        let md = "## Parent\n\nParent content.\n\n### Child\n\nChild content.\n";
        let sections = parse_markdown(md, "test");
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, "Parent");
        assert_eq!(sections[0].level, 2);
        assert_eq!(sections[1].heading, "Child");
        assert_eq!(sections[1].level, 3);
    }

    #[test]
    fn parse_content_before_heading() {
        let md = "Some intro text.\n\n## First Heading\n\nContent.\n";
        let sections = parse_markdown(md, "README");
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, "README");
        assert_eq!(sections[0].level, 0);
        assert_eq!(sections[0].content, "Some intro text.");
        assert_eq!(sections[1].heading, "First Heading");
    }

    #[test]
    fn parse_empty_file() {
        assert!(parse_markdown("", "test").is_empty());
    }

    #[test]
    fn parse_headings_only() {
        let md = "# Title\n## Sub\n";
        let sections = parse_markdown(md, "test");
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].content, "");
        assert_eq!(sections[1].content, "");
    }

    #[test]
    fn parse_consecutive_headings() {
        let md = "# A\n## B\n## C\n\nContent under C.\n";
        let sections = parse_markdown(md, "test");
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].content, "");
        assert_eq!(sections[1].content, "");
        assert!(sections[2].content.contains("Content under C."));
    }

    #[test]
    fn parse_heading_with_inline_code() {
        let md = "## The `foo` function\n\nDetails here.\n";
        let sections = parse_markdown(md, "test");
        assert_eq!(sections[0].heading, "The `foo` function");
    }

    #[test]
    fn parse_preserves_list_markers() {
        let md = "# Lists\n\n- alpha\n- beta\n- gamma\n";
        let sections = parse_markdown(md, "test");
        assert_eq!(sections[0].content, "- alpha\n- beta\n- gamma");
    }

    #[test]
    fn parse_preserves_numbered_list() {
        let md = "# Steps\n\n1. first\n2. second\n3. third\n";
        let sections = parse_markdown(md, "test");
        assert!(sections[0].content.contains("1. first"));
        assert!(sections[0].content.contains("3. third"));
    }

    #[test]
    fn parse_preserves_code_blocks() {
        let md = "# Code\n\n```rust\nfn main() {}\n```\n";
        let sections = parse_markdown(md, "test");
        assert!(sections[0].content.contains("```rust"));
        assert!(sections[0].content.contains("fn main() {}"));
    }

    #[test]
    fn parse_preserves_inline_formatting() {
        let md = "# Fmt\n\nSome **bold** and *italic* and `code`.\n";
        let sections = parse_markdown(md, "test");
        assert!(sections[0].content.contains("**bold**"));
        assert!(sections[0].content.contains("*italic*"));
        assert!(sections[0].content.contains("`code`"));
    }

    #[test]
    fn parse_preserves_blockquotes() {
        let md = "# Quote\n\n> Important note\n> continued\n";
        let sections = parse_markdown(md, "test");
        assert!(sections[0].content.contains("> Important note"));
    }

    #[test]
    fn parse_preserves_tables() {
        let md = "# Data\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
        let sections = parse_markdown(md, "test");
        assert!(sections[0].content.contains("| A | B |"));
    }

    #[test]
    fn base36_conversion() {
        assert_eq!(to_base36(0, 4), "0000");
        assert_eq!(to_base36(36, 4).chars().nth(1), Some('1'));
    }
}
