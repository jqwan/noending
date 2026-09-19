//! Workspace path identity — the ONE place a filesystem string becomes a
//! stable domain identity, and the ONE place a Project name is derived.
//!
//! Owned by Main. Wave 1+ agents must reuse these functions rather than write
//! a second normalizer: two normalizers means two `path_id`s for one directory,
//! and `workspace_paths.id` is the join key for Sessions, WorkstreamPaths and
//! therefore every Project fact.
//!
//! ## Why identity is lexical and never `fs::canonicalize`
//!
//! `canonical_path` is a UNIQUE identity column, so the same directory must
//! produce the same value forever. `std::fs::canonicalize` cannot promise that:
//! it fails while the directory does not exist yet (so a path that is created
//! later would flip identity), and a symlink may be introduced at any time by
//! something outside NoEnding. Both are disqualified for an identity key
//! (方案 §42.3-M8).
//!
//! The accepted cost is aliasing: `/tmp/x` and `/private/tmp/x`, or two spellings
//! that differ only by case on a case-insensitive volume, become two
//! WorkspacePath rows. That self-heals where it matters — Git detection resolves
//! both to the same `common_dir`, so §8.3 converges them into one Project — and
//! it is bounded: no path is ever silently rewritten underneath a user.

use sha2::{Digest, Sha256};
use std::fmt::Write;

/// Separator/case conventions. Every function here is platform-independent
/// except for which style it defaults to, so Windows behavior can be unit
/// tested on macOS (and vice versa) — CI runs both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    Unix,
    Windows,
}

impl PathStyle {
    pub fn current() -> Self {
        if cfg!(windows) {
            PathStyle::Windows
        } else {
            PathStyle::Unix
        }
    }

    pub fn is_windows(self) -> bool {
        matches!(self, PathStyle::Windows)
    }
}

/// Inputs that make normalization total (no ambient state).
#[derive(Debug, Clone, Copy, Default)]
pub struct NormalizeOpts<'a> {
    pub style: Option<PathStyle>,
    /// Base for relative paths. `None` means a relative input is rejected
    /// rather than guessed against `$HOME` or the process cwd — a wrong guess
    /// here would create a real WorkspacePath in the wrong place.
    ///
    /// Note that `..` is clamped *into* the base (`"../other"` becomes
    /// `"<base>/other"`), which is right for user input and wrong for evidence
    /// a tool handed you relative to some directory. Resolve those by joining
    /// first and normalizing after — do not widen this field's meaning.
    pub base: Option<&'a str>,
    /// Home directory used to expand `~` / `~\`. `None` disables expansion.
    pub home: Option<&'a str>,
}

/// Normalize `raw` for the current platform. Returns `None` when the input
/// cannot become an absolute path (empty, or relative with no base).
pub fn normalize_path(raw: &str) -> Option<String> {
    normalize_path_with(raw, NormalizeOpts::default())
}

/// See the module docs: pure function, no filesystem access, no OS queries.
pub fn normalize_path_with(raw: &str, opts: NormalizeOpts<'_>) -> Option<String> {
    let style = opts.style.unwrap_or_else(PathStyle::current);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // `~` expansion happens first: everything below assumes an absolute path
    // or a clearly-marked root. `~user` is deliberately unsupported (it would
    // need a passwd lookup, i.e. ambient state).
    let expanded = expand_tilde_with(trimmed, style, opts.home);
    let expanded = expanded.trim();
    if expanded.is_empty() {
        return None;
    }

    let (root, rest) = split_root(&expanded, style, opts.base)?;
    let segments = collapse_segments(&rest, style);
    Some(join(&root, &segments, style))
}

/// The separator-insensitive form used to derive `path_identity`. Backslashes
/// become `/`; nothing else changes (no case folding — see module docs).
pub fn path_key(canonical: &str) -> String {
    // Trailing separators are trimmed so a hand-written or externally-supplied
    // `/repo/x/` cannot hash to a second identity for `/repo/x`. `join` already
    // refuses to produce one; this covers anything that bypasses it, because a
    // split identity would give one directory two Projects. The bare root is
    // left alone — trimming it would give `""`.
    let trimmed = canonical.trim_end_matches(['/', '\\']);
    let base = if trimmed.is_empty() { canonical } else { trimmed };
    base.replace('\\', "/")
}

/// Deterministic WorkspacePath id for a canonical path.
///
/// A content id, not a uuid: `ensure_workspace_path` and the v12 backfill must
/// stay idempotent across a replayed migration (`migrate()` has no transaction),
/// and a random id would make every retry a new row that then has to be
/// reconciled. `path-` prefix keeps it visibly distinct from uuid v4 rows.
pub fn path_identity(canonical: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"noending:workspace-path:v1:");
    h.update(path_key(canonical).as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(37); // "path-" + 32
    out.push_str("path-");
    for b in digest.iter().take(16) {
        write!(out, "{:02x}", b).ok();
    }
    out
}

/// Identity of a raw path string, normalizing first. `None` if not absolute.
pub fn path_identity_of(raw: &str) -> Option<String> {
    normalize_path(raw).map(|c| path_identity(&c))
}

/// Last path segment, or the whole string for a bare root.
pub fn basename(canonical: &str) -> String {
    let key = path_key(canonical);
    let trimmed = key.trim_end_matches('/');
    // The bare root has no segment to name it, so the root is the name.
    if trimmed.is_empty() {
        return key;
    }
    match trimmed.rfind('/') {
        Some(i) => {
            let tail = &trimmed[i + 1..];
            if tail.is_empty() {
                trimmed.to_string()
            } else {
                tail.to_string()
            }
        }
        // Windows `C:\` (or `/`): the root itself carries the name.
        None => trimmed.to_string(),
    }
}

/// §37 automatic Project naming.
///
/// `default_workspace` is the absolute, normalized NoEnding default workspace;
/// only that exact path is called "NoEnding Workspace". A relocated NoEnding
/// Home leaves the old workspace as an ordinary path (§4), so it must NOT keep
/// the special name — which is why this is a parameter and not a
/// `ends_with("/.noending/workspace")` test.
///
/// Deviation from 方案 §37's example (`/Users/me/code/noending → NoEnding`):
/// only the first character is capitalized, so a lowercase directory yields
/// `Noending`. Recovering internal capitals would require a dictionary or a
/// Git remote name, both of which are exactly the "looks smarter, is a guess"
/// §41 forbids.
pub fn auto_project_name(canonical: &str, default_workspace: Option<&str>) -> String {
    let key = path_key(canonical);
    if let Some(dw) = default_workspace {
        if !dw.trim().is_empty() && path_key(dw.trim()) == key {
            return "NoEnding Workspace".to_string();
        }
    }
    let base = basename(&key);
    capitalize_first(&base)
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => {
            let mut out = c.to_uppercase().collect::<String>();
            out.push_str(chars.as_str());
            out
        }
        None => s.to_string(),
    }
}

/// Segment-wise containment: `is_within("/a/b/c", "/a/b")` is true, and
/// `is_within("/a/bc", "/a/b")` is false. A plain `str::starts_with` gets
/// exactly that second case wrong, and this predicate guards both the reserved
/// app paths (§2) and the Home-level Git exclusion (§1.4).
pub fn is_within(child: &str, ancestor: &str) -> bool {
    let child = path_key(child).trim_end_matches('/').to_string();
    let ancestor = path_key(ancestor).trim_end_matches('/').to_string();
    if child == ancestor {
        return true;
    }
    // A bare root ("/", "C:", "\\") already ends the boundary itself.
    let prefix = if ancestor.ends_with(':') {
        format!("{ancestor}/")
    } else if ancestor == "/" {
        "/".to_string()
    } else {
        format!("{ancestor}/")
    };
    child.starts_with(&prefix)
}

/// Expand a leading `~` / `~\` into `home`. This is the repository's only
/// tilde expander (方案 §42.3-M23); `platform::paths::expand_tilde` and
/// `launcher::expand_tilde` both delegate here.
///
/// Returns the input unchanged when there is no home, when the tilde is not
/// leading, or when it is a `~user` form (unsupported by design).
pub fn expand_tilde_with(raw: &str, style: PathStyle, home: Option<&str>) -> String {
    let t = raw.trim();
    // The guard is load-bearing on Windows: a UNC path starts with `\`, and
    // without this check `\\server\share` reads as "home-relative" and gets the
    // user's Home glued onto the front.
    if !t.starts_with('~') {
        return t.to_string();
    }
    let sep_after_tilde = match style {
        PathStyle::Windows => matches!(t.as_bytes().get(1), Some(b'\\') | Some(b'/')),
        PathStyle::Unix => t.as_bytes().get(1) == Some(&b'/'),
    };
    let rest = if t == "~" {
        ""
    } else if sep_after_tilde {
        &t[2..]
    } else {
        return t.to_string();
    };
    let Some(home) = home.map(str::trim).filter(|h| !h.is_empty()) else {
        return t.to_string();
    };
    if rest.is_empty() {
        return home.to_string();
    }
    let joiner = if style.is_windows() { '\\' } else { '/' };
    format!("{}{}{}", home.trim_end_matches(['/', '\\']), joiner, rest)
}

/// Same as [`expand_tilde_with`] with the platform's ambient home directory.
pub fn expand_tilde(raw: &str) -> String {
    let style = PathStyle::current();
    let home = home_dir().map(|p| p.to_string_lossy().to_string());
    expand_tilde_with(raw, style, home.as_deref())
}

pub fn home_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir()
}

// --------------------------------------------------------------------------
// internals
// --------------------------------------------------------------------------

/// Returns `(root_prefix, remainder)`. `root_prefix` is the platform's native
/// root spelling without a trailing separator (`/`, `C:`, `\\server\share`,
/// `\`); [`join`] adds the separator. `remainder` is always `/`-separated.
fn split_root(input: &str, style: PathStyle, base: Option<&str>) -> Option<(String, String)> {
    let s = strip_verbatim_prefix(input, style);

    // Drive-letter absolute: `C:\x` / `C:/x`. `C:x` is drive-*relative*, which
    // depends on that drive's current directory — ambient state, so reject it
    // instead of guessing.
    if s.len() >= 2 && s.as_bytes()[0].is_ascii_alphabetic() && s.as_bytes()[1] == b':' {
        let drive = s.as_bytes()[0].to_ascii_uppercase() as char;
        let rest = &s[2..];
        if !rest.starts_with('\\') && !rest.starts_with('/') {
            return None;
        }
        return Some((format!("{drive}:"), normalize_separators(rest)));
    }

    // UNC: `\\server\share\path`. The server+share pair IS the root and it
    // carries identity — dropping it would collide two different file servers.
    if style.is_windows() {
        if let Some(after) = s.replace('\\', "/").strip_prefix("//").map(str::to_string) {
            let parts: Vec<&str> = after.split('/').filter(|p| !p.is_empty()).collect();
            if parts.len() < 2 {
                return None;
            }
            let root = format!("\\\\{}\\{}", parts[0], parts[1]);
            return Some((root, parts[2..].join("/")));
        }
    }

    if s.starts_with('/') {
        return Some(("/".to_string(), normalize_separators(&s[1..])));
    }
    if style.is_windows() && s.starts_with('\\') {
        // `\foo` — root-relative with no drive. Kept as an explicit root so it
        // never silently becomes `$CWD\foo`.
        return Some(("\\".to_string(), normalize_separators(&s[1..])));
    }

    // Relative: needs an explicit base. No base, no answer.
    let base = base?;
    let (base_root, base_rest) = split_root(base, style, None)?;
    let base_segments = collapse_segments(&base_rest, style);
    let base_full = join(&base_root, &base_segments, style);
    Some((base_full, normalize_separators(&s)))
}

/// `root` + `segments`, with exactly one native separator between them.
fn join(root: &str, segments: &[String], style: PathStyle) -> String {
    let sep = if style.is_windows() { '\\' } else { '/' };
    let mut out = root.to_string();
    if segments.is_empty() {
        // `.` / `..` can collapse a path back onto its root. Return the root
        // alone: a trailing separator would give one directory two spellings in
        // a UNIQUE column — and therefore two WorkspacePaths and two Projects.
        // A bare drive root is the exception: `C:` alone is drive-*relative*,
        // a different directory than `C:\` whenever that drive's cwd is not its
        // root, so the separator is part of what makes it the root.
        if out.len() == 2 && out.ends_with(':') {
            out.push(sep);
        }
        return out;
    }
    if !out.ends_with('/') && !out.ends_with('\\') {
        out.push(sep);
    }
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            out.push(sep);
        }
        out.push_str(seg);
    }
    out
}

/// Windows' `\\?\` verbatim prefix is an API artifact, not part of the path:
/// it disables the very normalization we need and breaks `exists` checks. UNC
/// comes back as `\\server\share`.
fn strip_verbatim_prefix(input: &str, style: PathStyle) -> String {
    if !style.is_windows() {
        return input.to_string();
    }
    if let Some(rest) = input.strip_prefix("\\\\?\\UNC\\") {
        return format!("\\\\{}", rest);
    }
    if let Some(rest) = input.strip_prefix("\\\\?\\") {
        return rest.to_string();
    }
    input.to_string()
}

fn normalize_separators(rest: &str) -> String {
    rest.replace('\\', "/")
}

/// Drop `.` and empty segments, apply `..` without ever escaping the root.
/// Segment case is preserved verbatim on every platform: `canonical_path` is a
/// display string as well as an identity, and folding would mis-render it.
/// Case-insensitive volumes therefore alias, which is documented in the module
/// header as the accepted, Git-convergeable cost.
fn collapse_segments(rest: &str, _style: PathStyle) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for seg in rest.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                if !out.is_empty() {
                    out.pop();
                }
                // At the root, `..` is clamped: `/a/..` is `/a`, and `/..`
                // must not become a path outside `/`.
            }
            other => out.push(other.to_string()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unix(raw: &str) -> Option<String> {
        normalize_path_with(
            raw,
            NormalizeOpts {
                style: Some(PathStyle::Unix),
                base: None,
                home: Some("/Users/tester"),
            },
        )
    }

    fn win(raw: &str) -> Option<String> {
        normalize_path_with(
            raw,
            NormalizeOpts {
                style: Some(PathStyle::Windows),
                base: None,
                home: Some("C:\\Users\\tester"),
            },
        )
    }

    #[test]
    fn a_collapsed_path_never_grows_a_trailing_separator() {
        // `canonical_path` is a UNIQUE identity column, so `/repo/x` and
        // `/repo/x/` must never both be produced — otherwise one directory gets
        // two WorkspacePaths, and (through Project ownership) two homes.
        let base = NormalizeOpts {
            style: Some(PathStyle::Unix),
            base: Some("/repo/x"),
            home: Some("/Users/tester"),
        };
        assert_eq!(normalize_path_with(".", base).as_deref(), Some("/repo/x"));
        assert_eq!(
            normalize_path_with("./", base).as_deref(),
            Some("/repo/x")
        );
        assert_eq!(
            normalize_path_with("sub/..", base).as_deref(),
            Some("/repo/x")
        );
        let collapsed = normalize_path_with(".", base).map(|c| path_identity(&c));
        assert_eq!(collapsed.as_deref(), path_identity_of("/repo/x").as_deref());
        assert_eq!(
            collapsed.as_deref(),
            path_identity_of("/repo/x/").as_deref(),
            "the same directory resolves to the same id from either spelling"
        );
    }

    #[test]
    fn unix_normalization() {
        assert_eq!(unix("/a/b/c").as_deref(), Some("/a/b/c"));
        assert_eq!(unix("/a//b/./c/").as_deref(), Some("/a/b/c"));
        assert_eq!(unix("/a/b/../c").as_deref(), Some("/a/c"));
        assert_eq!(unix("/a/../../b").as_deref(), Some("/b"), "clamped at root");
        assert_eq!(unix("/").as_deref(), Some("/"));
        assert_eq!(unix("  /a/b  ").as_deref(), Some("/a/b"));
        assert_eq!(unix(""), None);
        assert_eq!(unix("relative/path"), None, "no base means no guess");
        assert_eq!(unix("~/code/x").as_deref(), Some("/Users/tester/code/x"));
        assert_eq!(unix("~").as_deref(), Some("/Users/tester"));
        assert_eq!(
            unix("/opt/a~b/c").as_deref(),
            Some("/opt/a~b/c"),
            "only leading ~"
        );
        assert_eq!(unix("~other/x"), None, "~user is unsupported, not expanded");
    }

    #[test]
    fn relative_path_needs_an_explicit_base() {
        let opts = NormalizeOpts {
            style: Some(PathStyle::Unix),
            base: Some("/Users/tester/projects"),
            home: Some("/Users/tester"),
        };
        assert_eq!(
            normalize_path_with("noending/src", opts).as_deref(),
            Some("/Users/tester/projects/noending/src")
        );
        assert_eq!(
            normalize_path_with("../other", opts).as_deref(),
            Some("/Users/tester/projects/other")
        );
    }

    #[test]
    fn windows_normalization() {
        assert_eq!(
            win("C:\\Users\\me\\code").as_deref(),
            Some("C:\\Users\\me\\code")
        );
        assert_eq!(
            win("c:/users/me/code/").as_deref(),
            Some("C:\\users\\me\\code"),
            "drive uppercased, rest kept"
        );
        assert_eq!(
            win("C:\\Users\\me\\code\\x\\..\\y").as_deref(),
            Some("C:\\Users\\me\\code\\y")
        );
        // The drive root keeps its separator: `C:` alone is drive-*relative*
        // on Windows and would not answer an `exists` check about the root.
        assert_eq!(win("C:\\").as_deref(), Some("C:\\"));
        assert_eq!(win("C:relative"), None, "drive-relative is not absolute");
        assert_eq!(
            win("\\\\?\\C:\\Users\\me").as_deref(),
            Some("C:\\Users\\me"),
            "verbatim prefix stripped"
        );
        assert_eq!(
            win("\\\\?\\UNC\\server\\share\\work").as_deref(),
            Some("\\\\server\\share\\work"),
            "UNC verbatim restores the leading \\\\"
        );
        assert_eq!(
            win("\\\\server\\share\\a\\b").as_deref(),
            Some("\\\\server\\share\\a\\b"),
            "UNC root keeps server+share"
        );
        assert_eq!(
            win("~\\.cargo\\x").as_deref(),
            Some("C:\\Users\\tester\\.cargo\\x")
        );
        assert_eq!(
            win("C:\\..\\..\\Windows").as_deref(),
            Some("C:\\Windows"),
            "clamped at drive root"
        );
    }

    #[test]
    fn separators_converge_into_one_identity() {
        // `git` and the Agent CLIs hand back mixed spellings for the same dir;
        // they must not become two WorkspacePaths.
        assert_eq!(
            path_identity(&win("C:/a/b").unwrap()),
            path_identity(&win("C:\\a\\b").unwrap())
        );
        assert_ne!(path_identity("/a/b"), path_identity("/a/B"));
        assert_eq!(path_identity("/a/b").len(), 5 + 32);
        assert!(path_identity("/a/b").starts_with("path-"));
        // Stable across processes: no random component.
        assert_eq!(path_identity("/a/b"), path_identity("/a/b"));
    }

    #[test]
    fn containment_is_segment_wise() {
        assert!(is_within("/a/b/c", "/a/b"));
        assert!(is_within("/a/b", "/a/b"));
        assert!(
            !is_within("/a/bc", "/a/b"),
            "string prefix is not containment"
        );
        assert!(!is_within("/a", "/a/b"));
        assert!(is_within("/a/b", "/"));
        assert!(is_within("C:\\a\\b", "C:"));
        assert!(!is_within("C:\\ab", "C:\\a"));
    }

    #[test]
    fn auto_naming() {
        assert_eq!(auto_project_name("/Users/me/research", None), "Research");
        assert_eq!(auto_project_name("/Users/me/noending", None), "Noending");
        assert_eq!(
            auto_project_name(
                "/Users/me/.noending/workspace",
                Some("/Users/me/.noending/workspace")
            ),
            "NoEnding Workspace"
        );
        assert_eq!(
            auto_project_name(
                "/Users/other/.noending/workspace",
                Some("/Users/me/.noending/workspace")
            ),
            "Workspace",
            "an old workspace after a Home move is an ordinary path"
        );
        assert_eq!(auto_project_name("/", None), "/");
        assert_eq!(auto_project_name("C:\\Users\\me", None), "Me");
        assert_eq!(auto_project_name("/Users/me/数据", None), "数据");
    }

    #[test]
    fn basename_and_root() {
        assert_eq!(basename("/a/b/c"), "c");
        assert_eq!(basename("/a/b/c/"), "c");
        assert_eq!(basename("/"), "/");
        assert_eq!(basename("C:\\a\\b"), "b");
    }
}
