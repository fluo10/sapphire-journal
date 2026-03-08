use std::path::Path;

use caretta_id::CarettaId;
use chrono::NaiveDateTime;

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

fn id_from_path(path: &Path) -> Option<CarettaId> {
    let stem = path.file_stem()?.to_str()?;
    stem.get(..7)?.parse().ok()
}

fn bare_frontmatter(path: &Path) -> Result<Frontmatter> {
    let id = id_from_path(path).ok_or_else(|| {
        Error::InvalidEntry(
            "no frontmatter and filename does not contain a valid CarettaId".into(),
        )
    })?;
    Ok(Frontmatter {
        id,
        title: String::new(),
        slug: None,
        created_at: NaiveDateTime::default(),
        updated_at: NaiveDateTime::default(),
        tags: Vec::new(),
        task: None,
        event: None,
    })
}

fn split_frontmatter<'a>(path: &Path, source: &'a str) -> Result<(Frontmatter, &'a str)> {
    let Some(rest) = source.strip_prefix(FENCE) else {
        return Ok((bare_frontmatter(path)?, source));
    };

    // The opening `---` must be followed by a newline.
    let Some(rest) = rest.strip_prefix('\n') else {
        return Ok((bare_frontmatter(path)?, source));
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
    fn parses_entry_without_frontmatter() {
        let src = "just a body\n";
        let entry = parse_entry(&managed_path(), src).unwrap();
        assert!(entry.frontmatter.title.is_empty());
        assert_eq!(entry.body, "just a body\n");
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
            closed_at: None,
        });
        let rendered = render_entry(&entry);
        assert!(rendered.contains("title: My Task"));
        assert!(rendered.contains("status: open"));
        assert!(rendered.contains("due: 2026-03-10"));
    }
}
