//! Notes helpers: safe relative paths, markdown headers, show/index views.
//!
//! All user-supplied note paths go through [`join_safe`]: absolute paths
//! and `..` never escape the storage root.

use std::fmt::Write as _;
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
    /// Header depth: 1 for `#`, 2 for `##`, and so on.
    pub level: usize,
    /// Header text without the `#` markers.
    pub title: String,
    /// 1-based line number inside the file.
    pub line: usize,
}

/// Opening fence marker (``` / ~~~, info string allowed): `(char, length)`.
fn fence_marker(trimmed: &str) -> Option<(char, usize)> {
    let ch = trimmed.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let len = trimmed.chars().take_while(|&c| c == ch).count();
    (len >= 3).then_some((ch, len))
}

/// All `#`-headers of a markdown text, in file order.
/// Lines inside fenced code blocks are not headers.
#[must_use]
pub fn read_headers(text: &str) -> Vec<MdHeader> {
    let mut out = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some((ch, len)) = fence_marker(trimmed) {
            match fence {
                Some((open, open_len)) if ch == open && len >= open_len => fence = None,
                None => fence = Some((ch, len)),
                _ => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let hashes = trimmed.len() - trimmed.trim_start_matches('#').len();
        if hashes == 0 || hashes > 6 {
            continue;
        }
        let rest = &trimmed[hashes..];
        if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
            continue;
        }
        let title = rest.trim().to_owned();
        if title.is_empty() {
            continue;
        }
        out.push(MdHeader {
            level: hashes,
            title,
            line: index + 1,
        });
    }
    out
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

/// Insert `body` at the end of the first section matching `header`.
/// `header` is a title (`Tasks`) or a `Parent/Child` path matched against
/// the header chain (first match in document order wins). Repeated inserts
/// keep chronological order with single blank separators.
pub fn insert_under(text: &str, header: &str, body: &str) -> anyhow::Result<String> {
    let want = header.trim();
    if want.is_empty() {
        return Err(ChoraError::NoteNoHeader(header.to_owned()).into());
    }
    let headers = read_headers(text);
    let mut stack: Vec<usize> = Vec::new();
    let mut target: Option<usize> = None;
    for (index, current) in headers.iter().enumerate() {
        while stack
            .last()
            .is_some_and(|&top| headers[top].level >= current.level)
        {
            stack.pop();
        }
        stack.push(index);
        let chain = stack
            .iter()
            .map(|&top| headers[top].title.as_str())
            .collect::<Vec<_>>()
            .join("/");
        if chain == want || chain.ends_with(&format!("/{want}")) {
            target = Some(index);
            break;
        }
    }
    let Some(found) = target else {
        return Err(ChoraError::NoteNoHeader(header.to_owned()).into());
    };
    let level = headers[found].level;
    let lines: Vec<&str> = text.lines().collect();
    let insert_at = headers
        .iter()
        .skip(found + 1)
        .find(|next| next.level <= level)
        .map(|next| next.line - 1)
        .unwrap_or(lines.len());
    let mut out: Vec<&str> = Vec::with_capacity(lines.len() + body.lines().count() + 2);
    out.extend_from_slice(&lines[..insert_at]);
    if !out.is_empty() && !out.last().is_some_and(|line| line.trim().is_empty()) {
        out.push("");
    }
    out.extend(body.lines());
    if insert_at < lines.len() {
        // The next line is a header: keep exactly one blank between.
        if !lines[insert_at].trim().is_empty() {
            out.push("");
        }
        out.extend_from_slice(&lines[insert_at..]);
    }
    let mut text = out.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

/// One note file with its effective metadata.
#[derive(Debug, Clone)]
pub struct FileView {
    /// Root-relative path (`docs/main.md`, or `now.md` for loose files).
    pub path: String,
    /// Effective security level for `path`.
    pub security: AccessLevel,
    /// Explicit `about:` or the file's own h1, else empty.
    pub about: String,
    /// Full body when requested, else empty.
    pub body: String,
}

/// One notes dir with its files (recursive).
#[derive(Debug, Clone)]
pub struct DirView {
    /// Dir name (`docs`), `.` for the notes root, `(files)` for loose files.
    pub dir: String,
    /// Effective security level for the dir itself.
    pub security: AccessLevel,
    /// Explicit `about:` of the dir entry, else empty.
    pub about: String,
    /// Visible files under the dir, recursive.
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
    if !security.permits(max) {
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

/// Resolve `--path` scopes under `dir` to a sorted, deduplicated file list.
/// Empty scopes = whole dir. Each scope is a file or a subdir inside.
pub fn collect_scoped(dir: &Path, scopes: &[String]) -> anyhow::Result<Vec<PathBuf>> {
    if scopes.is_empty() {
        return collect_md(dir);
    }
    let mut files = Vec::new();
    for rel in scopes {
        let scoped = join_safe(dir, rel)?;
        if scoped.is_dir() {
            files.extend(collect_md(&scoped)?);
        } else if scoped.is_file() {
            require_md(rel)?;
            files.push(scoped);
        } else {
            return Err(ChoraError::NoteBadPath(format!("{rel}: not found")).into());
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Views for `files` (absolute paths under `base`); `prefix` is prepended
/// to display paths (`dirname` for dirs, empty for the notes root).
fn file_views(
    index: &Index,
    base: &Path,
    prefix: &str,
    files: Vec<PathBuf>,
    max: AccessLevel,
    bodies: bool,
) -> anyhow::Result<Vec<FileView>> {
    let mut views = Vec::with_capacity(files.len());
    for abs in &files {
        let rel = if prefix.is_empty() {
            rel_of(base, abs)
        } else {
            format!("{prefix}/{}", rel_of(base, abs))
        };
        if let Some(view) = file_view(index, &rel, abs, max, bodies)? {
            views.push(view);
        }
    }
    Ok(views)
}

/// View of one notes subdir (recursive). Errors on unknown dirname.
pub fn dir_view(
    root: &Path,
    index: &Index,
    dirname: &str,
    max: AccessLevel,
    bodies: bool,
    scopes: &[String],
) -> anyhow::Result<DirView> {
    let notes = root.join("notes");
    let dir = join_safe(&notes, dirname)?;
    if !dir.is_dir() {
        return Err(ChoraError::NoteNoDir(dirname.to_owned()).into());
    }
    let (security, about) = (index.security_for(dirname), index.about_for(dirname));
    let files = file_views(
        index,
        &dir,
        dirname,
        collect_scoped(&dir, scopes)?,
        max,
        bodies,
    )?;
    Ok(DirView {
        dir: dirname.to_owned(),
        security,
        about: about.to_owned(),
        files,
    })
}

/// View of the notes root itself (`.` scope): subdirs and loose files.
/// `scopes` are root-relative; empty scopes = everything with bodies.
pub fn root_view(
    root: &Path,
    index: &Index,
    max: AccessLevel,
    bodies: bool,
    scopes: &[String],
) -> anyhow::Result<DirView> {
    let notes = root.join("notes");
    let files = file_views(
        index,
        &notes,
        "",
        collect_scoped(&notes, scopes)?,
        max,
        bodies,
    )?;
    Ok(DirView {
        dir: String::from("."),
        security: AccessLevel::Open,
        about: String::from("notes root"),
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
        // Loose `notes/*.md` files are entries too (`now.md`): they exist
        // as files, not dirs, so check accordingly.
        let exists = if top.ends_with(".md") {
            notes.join(top).is_file()
        } else {
            notes.join(top).is_dir()
        };
        if !exists {
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
        let _ = writeln!(
            out,
            "### {} [{}] - {}",
            dir.dir,
            dir.security.as_str(),
            describe(&dir.about)
        );
        for file in &dir.files {
            let _ = writeln!(
                out,
                "  {} [{}] - {}",
                file.path,
                file.security.as_str(),
                describe(&file.about)
            );
        }
    }
    if !loose.files.is_empty() {
        let _ = writeln!(out, "### {} - {}", loose.dir, loose.about);
        for file in &loose.files {
            let _ = writeln!(
                out,
                "  {} [{}] - {}",
                file.path,
                file.security.as_str(),
                describe(&file.about)
            );
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
        let _ = writeln!(
            out,
            "\n### {} (security: {})\nabout: {}",
            file.path,
            file.security.as_str(),
            describe(&file.about)
        );
        out.push_str(&file.body);
        if !file.body.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// `chora index events`: just the events guide, no clearance filtering.
#[must_use]
pub fn render_events() -> String {
    let mut out = String::from("# Events\n");
    out.push_str(Index::EVENTS_GUIDE);
    if !Index::EVENTS_GUIDE.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// `chora index`: context plus events guide plus visible entries.
#[must_use]
pub fn render_index(index: &Index, max: AccessLevel) -> String {
    let mut out = String::from("# Context\n");
    if index.context.is_empty() {
        out.push_str("(empty)\n");
    } else {
        out.push_str(&index.context);
        out.push('\n');
    }
    out.push_str("\n## Events\n");
    out.push_str(Index::EVENTS_GUIDE);
    if !Index::EVENTS_GUIDE.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("\n## Notes\n");
    let mut shown = false;
    for entry in index.visible(max) {
        let depth = entry.path.matches('/').count();
        let _ = writeln!(
            out,
            "{} {} [{}] - {}",
            "#".repeat(3 + depth),
            entry.path,
            entry.security.as_str(),
            describe(&entry.about)
        );
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
    fn insert_under_appends_at_section_end() {
        let text = "# T\n\n## A\nold\n\n## B\n";
        let out = insert_under(text, "A", "new\n").unwrap();
        assert_eq!(out, "# T\n\n## A\nold\n\nnew\n\n## B\n");
    }

    #[test]
    fn insert_under_repeats_keep_order_without_blank_pileup() {
        let text = "# T\n\n## A\nold\n\n## B\n";
        let once = insert_under(text, "A", "one").unwrap();
        let twice = insert_under(&once, "A", "two").unwrap();
        assert_eq!(twice, "# T\n\n## A\nold\n\none\n\ntwo\n\n## B\n");
    }

    #[test]
    fn insert_under_targets_duplicates_by_parent_child_path() {
        let text = "# T\n## A\n### K\none\n## B\n### K\ntwo\n";
        let out = insert_under(text, "B/K", "new").unwrap();
        assert!(out.contains("### K\ntwo\n\nnew\n"), "{out}");
        assert!(out.contains("### K\none\n## B"), "{out}");
    }

    #[test]
    fn insert_under_ignores_tags_and_fenced_headers() {
        let text = "# T\n#tag\n\n```sh\n# not a header\n```\n\n## Real\n";
        assert!(insert_under(text, "tag", "x").is_err());
        assert!(insert_under(text, "not a header", "x").is_err());
        let out = insert_under(text, "Real", "x").unwrap();
        assert!(out.contains("## Real\n\nx\n"), "{out}");
    }

    #[test]
    fn headers_skip_fenced_code_blocks() {
        let text =
            "# Top\n\n```sh\n# fake\n## still fake\n```\n\n## Real\n\n~~~\n# also fake\n~~~\n";
        let headers = read_headers(text);
        assert_eq!(
            headers.iter().map(|h| h.title.as_str()).collect::<Vec<_>>(),
            vec!["Top", "Real"]
        );
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

    #[test]
    fn render_index_embeds_events_guide() {
        let index =
            Index::parse("# Context\n\nHi.\n\n## Notes\n\n### docs\nsecurity: open\nabout: Docs\n")
                .unwrap();
        let text = render_index(&index, AccessLevel::Open);
        assert!(text.contains("# Context"));
        assert!(text.contains("## Events"));
        assert!(text.contains("monday"));
        assert!(text.contains("## Notes"));
        assert!(text.contains("docs"));
    }

    #[test]
    fn render_events_is_just_the_guide() {
        let text = render_events();
        assert!(text.starts_with("# Events\n"));
        assert!(text.contains("monday"));
        assert!(text.contains("sunday"));
    }

    #[test]
    fn unknown_dir_warnings_ignore_existing_loose_files() {
        let tmp = tempfile::tempdir().unwrap();
        let notes = tmp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(notes.join("now.md"), "# Now\n").unwrap();
        std::fs::create_dir_all(notes.join("docs")).unwrap();
        let index = Index::parse("## Notes\n\n### now.md\nabout: Loose\n\n### docs\nabout: Docs\n")
            .unwrap();
        assert!(unknown_dir_warnings(&index, &notes).is_empty());
    }

    #[test]
    fn unknown_dir_warnings_still_fire_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let notes = tmp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let index = Index::parse("## Notes\n\n### now.md\nabout: Loose\n\n### docs\nabout: Docs\n")
            .unwrap();
        assert_eq!(unknown_dir_warnings(&index, &notes).len(), 2);
    }
}
