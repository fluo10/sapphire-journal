use std::path::Path;

use crate::{
    entry::{Entry, Frontmatter},
    error::{Error, Result},
};

const FENCE: &str = "---";

/// Parse a Markdown file into an [`Entry`].
///
/// Frontmatter is optional. If the file starts with `---`, everything until
/// the closing `---` is parsed as YAML. The rest is the body.
pub fn parse_entry(path: &Path, source: &str) -> Result<Entry> {
    let (frontmatter, body) = split_frontmatter(path, source)?;
    Ok(Entry {
        path: path.to_path_buf(),
        frontmatter,
        body: body.to_owned(),
    })
}

/// Read a file from disk and parse it.
pub fn read_entry(path: &Path) -> Result<Entry> {
    let source = std::fs::read_to_string(path)?;
    parse_entry(path, &source)
}

fn split_frontmatter<'a>(_path: &Path, source: &'a str) -> Result<(Frontmatter, &'a str)> {
    let Some(rest) = source.strip_prefix(FENCE) else {
        return Err(Error::InvalidEntry("missing frontmatter block".into()));
    };

    // The opening `---` must be followed by a newline.
    let Some(rest) = rest.strip_prefix('\n') else {
        return Err(Error::InvalidEntry("missing frontmatter block".into()));
    };

    let Some(end) = rest.find(&format!("\n{FENCE}")) else {
        return Err(Error::InvalidEntry(
            "frontmatter block is not closed".into(),
        ));
    };

    let yaml = &rest[..end];
    let body = &rest[end + 1 + FENCE.len()..]; // skip `\n---`
    let body = body.trim_start_matches('\n');

    let frontmatter: Frontmatter = serde_yaml::from_str(yaml)?;
    Ok((frontmatter, body))
}

/// Serialize an [`Entry`] back to Markdown source.
pub fn render_entry(entry: &Entry) -> String {
    let mut out = String::new();

    let yaml =
        serde_yaml::to_string(&entry.frontmatter).expect("frontmatter serialization failed");
    out.push_str("---\n");
    out.push_str(&yaml);
    out.push_str("---\n");
    if !entry.body.is_empty() {
        out.push('\n');
    }

    out.push_str(&entry.body);
    out
}

/// Write an [`Entry`] back to its source file, updating `updated_at` first.
pub fn write_entry(entry: &mut Entry) -> Result<()> {
    entry.frontmatter.updated_at = chrono::Local::now().naive_local();
    std::fs::write(&entry.path, render_entry(entry))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use std::path::PathBuf;

    /// A valid archelon-managed path with CarettaId "0000000" (NIL).
    fn managed_path() -> PathBuf {
        PathBuf::from("0000000_test.md")
    }

    #[test]
    fn parses_entry_with_frontmatter() {
        let src = "---\nid: '0000000'\ntitle: Hello\ntags: [rust, cli]\n---\nsome body\n";
        let entry = parse_entry(&managed_path(), src).unwrap();
        assert_eq!(entry.frontmatter.title, "Hello");
        assert_eq!(entry.frontmatter.tags, vec!["rust", "cli"]);
        assert_eq!(entry.body, "some body\n");
    }

    #[test]
    fn renders_entry_with_task() {
        use crate::entry::TaskMeta;
        let src = "---\nid: '0000000'\n---\nbody\n";
        let mut entry = parse_entry(&managed_path(), src).unwrap();
        entry.frontmatter.title = "My Task".into();
        entry.frontmatter.task = Some(TaskMeta {
            status: "open".into(),
            due: Some("2026-03-10T00:00:00".parse().unwrap()),
            started_at: None,
            closed_at: None,
            extra: IndexMap::new(),
        });
        let rendered = render_entry(&entry);
        assert!(rendered.contains("title: My Task"));
        assert!(rendered.contains("status: open"));
        assert!(rendered.contains("due: 2026-03-10"));
    }
}
