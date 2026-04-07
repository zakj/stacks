use std::io::IsTerminal;

use crate::store::Chunk;

const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";
const RULE_WIDTH: usize = 72;

const TREE_BRANCH: &str = "├─ ";
const TREE_LAST: &str = "└─ ";
const TREE_PIPE: &str = "│  ";
const TREE_SPACE: &str = "   ";

pub struct Formatter {
    use_color: bool,
}

impl Formatter {
    pub fn new() -> Self {
        Formatter {
            use_color: std::io::stdout().is_terminal(),
        }
    }

    pub fn dim(&self, text: &str) -> String {
        if self.use_color {
            format!("{DIM}{text}{RESET}")
        } else {
            text.to_string()
        }
    }

    pub fn bold(&self, text: &str) -> String {
        if self.use_color {
            format!("{BOLD}{text}{RESET}")
        } else {
            text.to_string()
        }
    }

    pub fn slug(&self, slug: &str) -> String {
        self.dim(&format!("[{slug}]"))
    }

    pub fn tree_connector(&self, is_last: bool) -> String {
        self.dim(if is_last { TREE_LAST } else { TREE_BRANCH })
    }

    pub fn tree_indent(&self, has_more: bool) -> String {
        self.dim(if has_more { TREE_PIPE } else { TREE_SPACE })
    }

    /// Leading rule: `── heading`
    pub fn section_rule(&self, heading: &str) -> String {
        format!("{} {}", self.dim("──"), self.bold(heading))
    }

    /// Box-top rule: `── source_path ─────────────`
    /// Rule chars dim, source path bold.
    pub fn box_top(&self, source_path: &str) -> String {
        let label = self.bold(source_path);
        let used = 3 + source_path.chars().count() + 1; // "── " + path + " "
        let fill_len = RULE_WIDTH.saturating_sub(used).max(2);
        let fill = self.dim(&"─".repeat(fill_len));
        format!("{} {label} {fill}", self.dim("──"))
    }

    /// Breadcrumb with bold headings and dim `>` separators.
    pub fn breadcrumb(&self, ancestors: &[Chunk], heading: &str) -> String {
        let sep = self.dim(" > ");
        let parts: Vec<String> = ancestors
            .iter()
            .map(|a| self.bold(&a.heading))
            .chain(std::iter::once(self.bold(heading)))
            .collect();
        parts.join(&sep)
    }

    /// Breadcrumb with immediate parent slug for navigation.
    pub fn breadcrumb_nav(&self, ancestors: &[Chunk], heading: &str) -> String {
        let sep = self.dim(" > ");
        let mut parts = Vec::new();
        for (i, ancestor) in ancestors.iter().enumerate() {
            if i == ancestors.len() - 1 {
                parts.push(format!(
                    "{} {}",
                    self.bold(&ancestor.heading),
                    self.slug(&ancestor.slug)
                ));
            } else {
                parts.push(self.bold(&ancestor.heading));
            }
        }
        parts.push(self.bold(heading));
        parts.join(&sep)
    }
}

/// Strip slug delimiters so `show` accepts bare slugs, `[slug]`, or `path#slug`.
pub fn parse_slug(target: &str) -> &str {
    let s = target.rsplit('#').next().unwrap_or(target);
    s.strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_slug_formats() {
        assert_eq!(parse_slug("a3f2"), "a3f2");
        assert_eq!(parse_slug("[a3f2]"), "a3f2");
        assert_eq!(parse_slug("docs/patterns.md#a3f2"), "a3f2");
        assert_eq!(parse_slug("docs/patterns.md#[a3f2]"), "a3f2");
    }
}
