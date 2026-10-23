//! Notes helpers: safe relative paths, markdown headers, show/index views.
//!
//! All user-supplied note paths go through [`join_safe`]: absolute paths
//! and `..` never escape the storage root.

use std::path::{Component, Path, PathBuf};

use anyhow::Context;

use crate::error::ChoraError;
use crate::index::{AccessLevel, Index, Warning};

/// Join a user-supplied relative path onto `base`.
/// Rejects absolute paths and anything but normal segments (`..`, `.`).
pub fn join_safe(base: &Path, rel: &str) -> anyhow::Result<PathBuf> {
    if rel.trim().is_empty() {
        return Err(ChoraError::NoteBadPath(String::from("(empty path)")).into());
    }
    let mut out = base.to_path_buf();
    let mut pushed = false;
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => {
                out.push(part);
                pushed = true;
            }
            _ => return Err(ChoraError::NoteBadPath(rel.to_owned()).into()),
        }
    }
    if !pushed {
        return Err(ChoraError::NoteBadPath(rel.to_owned()).into());
    }
    Ok(out)
}

/// Note files must end with `.md`.
pub fn require_md(rel: &str) -> anyhow::Result<()> {
    if rel.ends_with(".md") {
        Ok(())
    } else {
        Err(ChoraError::NoteBadPath(format!("{rel}: path must end with .md")).into())
    }
}

/// One `#`-header inside a note file (1-based line number).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdHeader {
    pub level: usize,
    pub title: String,
    pub line: usize,
}

/// All `#`-headers of a markdown text, in file order.
#[must_use]
pub fn read_headers(text: &str) -> Vec<MdHeader> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim_start();
            let hashes = trimmed.len() - trimmed.trim_start_matches('#').len();
            if hashes == 0 || hashes > 6 {
                return None;
            }
            let rest = &trimmed[hashes..];
            if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
                return None;
            }
            let title = rest.trim().to_owned();
            if title.is_empty() {
                return None;
            }
            Some(MdHeader {
                level: hashes,
                title,
                line: index + 1,
            })
        })
        .collect()
}

/// The file's own h1 (`# Guide` → `Guide`), if any.
#[must_use]
pub fn first_h1(text: &str) -> Option<String> {
    read_headers(text)
        .into_iter()
        .find(|header| header.level == 1)
        .map(|header| header.title)
}

/// All `.md` files under `dir`, recursive, sorted. Missing dir = empty.
pub fn collect_md(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("failed to list {}", current.display()));
            }
        };
        for entry in entries {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "md") {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// `/`-joined path of `abs` relative to `base`.
#[must_use]
pub fn rel_of(base: &Path, abs: &Path) -> String {
    abs.strip_prefix(base)
        .map(|rel| {
            rel.to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/")
        })
        .unwrap_or_default()
}

/// Insert `body` directly under the first header titled `header`.
pub fn insert_under(text: &str, header: &str, body: &str) -> anyhow::Result<String> {
    let want = header.trim();
    let lines: Vec<&str> = text.lines().collect();
    let position = lines.iter().position(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with('#') && trimmed.trim_start_matches('#').trim() == want
    });
    let Some(index) = position else {
        return Err(ChoraError::NoteNoHeader(header.to_owned()).into());
    };
    let mut out: Vec<&str> = Vec::with_capacity(lines.len() + body.lines().count() + 2);
    out.extend_from_slice(&lines[..=index]);
    out.push("");
    for line in body.lines() {
        out.push(line);
    }
    out.extend_from_slice(&lines[index + 1..]);
    let mut text = out.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

/// One note file with its effective metadata.
#[derive(Debug, Clone)]
pub struct FileView {
    pub path: String,
    pub security: AccessLevel,
    pub about: String,
    pub body: String,
}

/// One notes dir with its files (recursive).
#[derive(Debug, Clone)]
pub struct DirView {
    pub dir: String,
    pub security: AccessLevel,
    pub about: String,
    pub files: Vec<FileView>,
}

/// Sorted top-level subdir names of `notes/`. Missing dir = empty.
pub fn top_dirs(notes: &Path) -> anyhow::Result<Vec<String>> {
    let mut dirs = Vec::new();
    let entries = match std::fs::read_dir(notes) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(dirs),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to list {}", notes.display()));
        }
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Sorted top-level `.md` file names of `notes/` (loose files).
pub fn top_files(notes: &Path) -> anyhow::Result<Vec<String>> {
    let mut files = Vec::new();
    let entries = match std::fs::read_dir(notes) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to list {}", notes.display()));
        }
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_file() && name.ends_with(".md") {
            files.push(name);
        }
    }
    files.sort();
    Ok(files)
}

fn file_view(
    index: &Index,
    rel: &str,
    abs: &Path,
    max: AccessLevel,
    bodies: bool,
) -> anyhow::Result<Option<FileView>> {
    let (security, index_about) = (index.security_for(rel), index.about_for(rel));
    if !security.allows(max) {
        return Ok(None);
    }
    // Explicit index.md `about` wins; otherwise fall back to the file's own
    // h1. The bare listing reads files lacking an `about` for that reason.
    let text = if bodies || index_about.is_empty() {
        std::fs::read_to_string(abs).with_context(|| format!("failed to read {}", abs.display()))?
    } else {
        String::new()
    };
    let about = if index_about.is_empty() {
        first_h1(&text).unwrap_or_default()
    } else {
        index_about.to_owned()
    };
    let body = if bodies { text } else { String::new() };
    Ok(Some(FileView {
        path: rel.to_owned(),
        security,
        about: about.to_owned(),
        body,
    }))
}

/// View of one notes subdir (recursive). Errors on unknown dirname.
pub fn dir_view(
    root: &Path,
    index: &Index,
    dirname: &str,
    max: AccessLevel,
    bodies: bool,
) -> anyhow::Result<DirView> {
    let notes = root.join("notes");
    let dir = join_safe(&notes, dirname)?;
    if !dir.is_dir() {
        return Err(ChoraError::NoteNoDir(dirname.to_owned()).into());
    }
    let (security, about) = (index.security_for(dirname), index.about_for(dirname));
    let mut files = Vec::new();
    for abs in collect_md(&dir)? {
        let rel = format!("{dirname}/{}", rel_of(&dir, &abs));
        if let Some(view) = file_view(index, &rel, &abs, max, bodies)? {
            files.push(view);
        }
    }
    Ok(DirView {
        dir: dirname.to_owned(),
        security,
        about: about.to_owned(),
        files,
    })
}

/// View of loose top-level `notes/*.md` files (always `open`).
pub fn loose_files_view(
    root: &Path,
    index: &Index,
    max: AccessLevel,
    bodies: bool,
) -> anyhow::Result<DirView> {
    let notes = root.join("notes");
    let mut files = Vec::new();
    for name in top_files(&notes)? {
        let abs = notes.join(&name);
        if let Some(view) = file_view(index, &name, &abs, max, bodies)? {
            files.push(view);
        }
    }
    Ok(DirView {
        dir: String::from("(files)"),
        security: AccessLevel::Open,
        about: String::from("loose files directly under notes/"),
        files,
    })
}

/// Warnings for index entries whose top-level dir is not on disk.
#[must_use]
pub fn unknown_dir_warnings(index: &Index, notes: &Path) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let mut seen = Vec::new();
    for entry in &index.entries {
        let top = entry.path.split('/').next().unwrap_or("");
        if top.is_empty() || top == "(files)" || seen.contains(&top) {
            continue;
        }
        seen.push(top);
        if !notes.join(top).is_dir() {
            warnings.push(Warning {
                path: Some(entry.path.clone()),
                msg: format!("unknown dirname `{top}`: not in notes/"),
            });
        }
    }
    warnings
}

fn describe(about: &str) -> &str {
    if about.is_empty() {
        "(no description)"
    } else {
        about
    }
}

/// Bare `note show`: dirs and files with metadata, no bodies.
#[must_use]
pub fn render_bare(dirs: &[DirView], loose: &DirView) -> String {
    let mut out = String::new();
    for dir in dirs {
        out.push_str(&format!(
            "### {} [{}] - {}\n",
            dir.dir,
            dir.security.as_str(),
            describe(&dir.about)
        ));
        for file in &dir.files {
            out.push_str(&format!(
                "  {} [{}] - {}\n",
                file.path,
                file.security.as_str(),
                describe(&file.about)
            ));
        }
    }
    if !loose.files.is_empty() {
        out.push_str(&format!("### {} - {}\n", loose.dir, loose.about));
        for file in &loose.files {
            out.push_str(&format!(
                "  {} [{}] - {}\n",
                file.path,
                file.security.as_str(),
                describe(&file.about)
            ));
        }
    }
    if out.is_empty() {
        out.push_str("(no notes)\n");
    }
    out
}

/// `note show <dirname>`: metadata plus full bodies, recursive.
#[must_use]
pub fn render_dir(dir: &DirView) -> String {
    let mut out = format!(
        "## notes/{} (security: {})\nabout: {}\n",
        dir.dir,
        dir.security.as_str(),
        describe(&dir.about)
    );
    if dir.files.is_empty() {
        out.push_str("(no visible notes)\n");
        return out;
    }
    for file in &dir.files {
        out.push_str(&format!(
            "\n### {} (security: {})\nabout: {}\n",
            file.path,
            file.security.as_str(),
            describe(&file.about)
        ));
        out.push_str(&file.body);
        if !file.body.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// `chora index`: context plus visible entries, original header levels.
#[must_use]
pub fn render_index(index: &Index, max: AccessLevel) -> String {
    let mut out = String::from("# Context\n");
    if index.context.is_empty() {
        out.push_str("(empty)\n");
    } else {
        out.push_str(&index.context);
        out.push('\n');
    }
    out.push_str("\n## Notes\n");
    let mut shown = false;
    for entry in index.visible(max) {
        let depth = entry.path.matches('/').count();
        out.push_str(&format!(
            "{} {} [{}] - {}\n",
            "#".repeat(3 + depth),
            entry.path,
            entry.security.as_str(),
            describe(&entry.about)
        ));
        shown = true;
    }
    if !shown {
        out.push_str("(no visible entries)\n");
    }
    out
}

fn file_json(file: &FileView, with_body: bool) -> serde_json::Value {
    let mut item = serde_json::json!({
        "path": file.path,
        "security": file.security.as_str(),
        "about": file.about,
    });
    if with_body {
        item["body"] = serde_json::Value::String(file.body.clone());
    }
    item
}

/// JSON for bare `note show --json`: metadata, no bodies.
#[must_use]
pub fn bare_json(dirs: &[DirView], loose: &DirView) -> serde_json::Value {
    let dirs: Vec<serde_json::Value> = dirs
        .iter()
        .map(|dir| {
            serde_json::json!({
                "dir": dir.dir,
                "security": dir.security.as_str(),
                "about": dir.about,
                "files": dir.files.iter().map(|f| file_json(f, false)).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({
        "dirs": dirs,
        "loose": loose.files.iter().map(|f| file_json(f, false)).collect::<Vec<_>>(),
    })
}

/// JSON for `note show <dirname> --json`: metadata plus bodies.
#[must_use]
pub fn dir_json(dir: &DirView) -> serde_json::Value {
    serde_json::json!({
        "dir": dir.dir,
        "security": dir.security.as_str(),
        "about": dir.about,
        "files": dir.files.iter().map(|f| file_json(f, true)).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn join_safe_accepts_relative_paths() {
        let base = PathBuf::from("/tmp/root");
        assert_eq!(
            join_safe(&base, "docs/main.md").unwrap(),
            PathBuf::from("/tmp/root/docs/main.md")
        );
        assert_eq!(
            join_safe(&base, "docs").unwrap(),
            PathBuf::from("/tmp/root/docs")
        );
    }

    #[test]
    fn join_safe_rejects_escapes() {
        let base = PathBuf::from("/tmp/root");
        for rel in ["", "../x", "a/../../x", "/abs/path", ".."] {
            assert!(join_safe(&base, rel).is_err(), "{rel}");
        }
        // `.` segments normalize away inside the base: harmless.
        assert_eq!(
            join_safe(&base, "a/./b").unwrap(),
            PathBuf::from("/tmp/root/a/b")
        );
    }

    #[test]
    fn require_md_enforces_extension() {
        assert!(require_md("main.md").is_ok());
        assert!(require_md("main.txt").is_err());
        assert!(require_md("main").is_err());
    }

    #[test]
    fn headers_extract_levels_titles_and_lines() {
        let text = "# Top\n\ntext\n## Mid\n### Deep\nno-space # not a header\n#Nope\n";
        let headers = read_headers(text);
        assert_eq!(
            headers,
            vec![
                MdHeader {
                    level: 1,
                    title: String::from("Top"),
                    line: 1
                },
                MdHeader {
                    level: 2,
                    title: String::from("Mid"),
                    line: 4
                },
                MdHeader {
                    level: 3,
                    title: String::from("Deep"),
                    line: 5
                },
            ]
        );
    }

    #[test]
    fn insert_under_places_body_after_header() {
        let text = "# T\n\n## A\nold\n\n## B\n";
        let out = insert_under(text, "A", "new\n").unwrap();
        assert!(out.contains("## A\n\nnew\nold"));
        assert!(out.contains("## B"));
    }

    #[test]
    fn insert_under_rejects_unknown_header() {
        assert!(insert_under("# T\n", "Missing", "x").is_err());
    }

    #[test]
    fn first_h1_picks_level_one_only() {
        assert_eq!(first_h1("# Guide\n\n## Tasks\n").as_deref(), Some("Guide"));
        assert_eq!(first_h1("## Only h2\n"), None);
        assert_eq!(first_h1("plain text\n"), None);
        assert_eq!(first_h1(""), None);
    }
}
