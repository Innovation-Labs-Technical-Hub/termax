//! Finds `/// Verifies: <scenario>` doc lines in Rust sources and attaches
//! them to the `#[test]` functions they document.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

use super::specs::normalize;

/// Prefix of a tag line, after the doc-comment marker.
pub const TAG_PREFIX: &str = "Verifies:";

/// A `Verifies:` doc line somewhere in the sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// Normalized text after `Verifies:`, e.g. `FR-AG03-001 acceptance 1`.
    pub scenario: String,
    pub file: PathBuf,
    pub line: usize,
}

/// A test function carrying at least one tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedTest {
    /// Root file of the cargo target the test compiles into
    /// (`crates/x/src/lib.rs`, `crates/x/tests/foo.rs`, …).
    pub target_root: PathBuf,
    /// Test name as libtest prints it: module path within the target plus
    /// the function name (`tests::denies_rm_rf`).
    pub name: String,
    pub file: PathBuf,
    pub line: usize,
    pub tags: Vec<Tag>,
}

/// Result of scanning a source tree.
#[derive(Debug, Default)]
pub struct Scan {
    pub tests: Vec<TaggedTest>,
    /// Tags not attached to a test function in a known cargo target, with
    /// the reason.
    pub stray: Vec<(Tag, &'static str)>,
}

/// Scan every `.rs` file under `crates_dir` (skipping `target` and hidden
/// directories).
pub fn scan(crates_dir: &Path) -> Result<Scan> {
    let mut files = Vec::new();
    collect_rs_files(crates_dir, &mut files)?;
    files.sort();
    let mut out = Scan::default();
    for file in files {
        let source = std::fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?;
        // Cheap pre-filter: most files carry no tags at all.
        if source.contains(TAG_PREFIX) {
            scan_file(&file, &source, &mut out)?;
        }
    }
    Ok(out)
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("listing {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("inspecting {}", path.display()))?;
        if file_type.is_dir() {
            let name = entry.file_name();
            if name != "target" && !name.to_string_lossy().starts_with('.') {
                collect_rs_files(&path, out)?;
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// Scan one file's source. `file` locates the cargo target and module path
/// on disk; its contents are taken from `source`.
pub fn scan_file(file: &Path, source: &str, out: &mut Scan) -> Result<()> {
    let ast = syn::parse_file(source).with_context(|| format!("parsing {}", file.display()))?;
    let location = locate(file);
    let mut visitor = Visitor {
        file,
        module: location
            .as_ref()
            .map(|l| l.module.clone())
            .unwrap_or_default(),
        tests: Vec::new(),
        seen: Vec::new(),
        attached: HashSet::new(),
    };
    visitor.visit_file(&ast);

    out.stray.extend(
        visitor
            .seen
            .iter()
            .filter(|t| !visitor.attached.contains(&t.line))
            .map(|t| (t.clone(), "not on a #[test] function")),
    );
    match location {
        Some(loc) => {
            out.tests.extend(
                visitor
                    .tests
                    .into_iter()
                    .map(|(name, line, tags)| TaggedTest {
                        target_root: loc.target_root.clone(),
                        name,
                        file: file.to_path_buf(),
                        line,
                        tags,
                    }),
            )
        }
        None => out.stray.extend(
            visitor
                .tests
                .into_iter()
                .flat_map(|(_, _, tags)| tags)
                .map(|t| (t, "test is not in a src/ or tests/ cargo target")),
        ),
    }
    Ok(())
}

struct Visitor<'a> {
    file: &'a Path,
    module: Vec<String>,
    /// (test name, line, tags) for tagged test functions.
    tests: Vec<(String, usize, Vec<Tag>)>,
    /// Every tag line seen, attached or not.
    seen: Vec<Tag>,
    /// Lines of tags that belong to a test function.
    attached: HashSet<usize>,
}

impl Visitor<'_> {
    fn tag(&self, attr: &syn::Attribute) -> Option<Tag> {
        let text = doc_text(attr)?;
        let scenario = text.trim().strip_prefix(TAG_PREFIX)?;
        Some(Tag {
            scenario: normalize(scenario),
            file: self.file.to_path_buf(),
            line: attr.span().start().line,
        })
    }
}

impl<'ast> Visit<'ast> for Visitor<'_> {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        self.module.push(node.ident.to_string());
        visit::visit_item_mod(self, node);
        self.module.pop();
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if is_test(&node.attrs) {
            let tags: Vec<Tag> = node.attrs.iter().filter_map(|a| self.tag(a)).collect();
            if !tags.is_empty() {
                self.attached.extend(tags.iter().map(|t| t.line));
                let mut path = self.module.clone();
                path.push(node.sig.ident.to_string());
                let line = node.sig.ident.span().start().line;
                self.tests.push((path.join("::"), line, tags));
            }
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        if let Some(tag) = self.tag(attr) {
            self.seen.push(tag);
        }
        visit::visit_attribute(self, attr);
    }
}

/// `#[test]`, `#[tokio::test]`, `#[tokio::test(flavor = …)]`, …
fn is_test(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| a.path().segments.last().is_some_and(|s| s.ident == "test"))
}

/// Text of a `///` or `//!` line (`#[doc = "…"]`).
fn doc_text(attr: &syn::Attribute) -> Option<String> {
    if !attr.path().is_ident("doc") {
        return None;
    }
    match &attr.meta {
        syn::Meta::NameValue(syn::MetaNameValue {
            value:
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }),
            ..
        }) => Some(s.value()),
        _ => None,
    }
}

/// Where a file sits in its crate.
#[derive(Debug, PartialEq, Eq)]
struct Location {
    target_root: PathBuf,
    module: Vec<String>,
}

/// Map `<pkg>/src/a/b.rs` to the lib root (or the bin root, for a crate
/// without `lib.rs`) and module `a::b`, and `<pkg>/tests/foo.rs` (or
/// `tests/foo/…` with a `main.rs`) to the integration test target `foo`.
/// `<pkg>` is the nearest ancestor holding a `Cargo.toml`. Returns `None`
/// for files outside such targets (examples, build scripts, shared test
/// modules like `tests/common/mod.rs`).
fn locate(file: &Path) -> Option<Location> {
    let package = file
        .ancestors()
        .skip(1)
        .find(|dir| dir.join("Cargo.toml").is_file())?;
    let inner: Vec<&str> = file
        .strip_prefix(package)
        .ok()?
        .iter()
        .map(|c| c.to_str())
        .collect::<Option<_>>()?;
    match inner.as_slice() {
        ["src", rest @ ..] => {
            let lib = package.join("src/lib.rs");
            let main = package.join("src/main.rs");
            let target_root = if !lib.is_file() && main.is_file() {
                main
            } else {
                lib
            };
            Some(Location {
                target_root,
                module: module_path(rest)?,
            })
        }
        ["tests", _single] => Some(Location {
            target_root: file.to_path_buf(),
            module: Vec::new(),
        }),
        ["tests", dir, rest @ ..] => {
            let main = package.join("tests").join(dir).join("main.rs");
            if !main.is_file() {
                return None;
            }
            Some(Location {
                target_root: main,
                module: module_path(rest)?,
            })
        }
        _ => None,
    }
}

/// Module path of a file relative to its target root's directory:
/// `lib.rs`/`main.rs` → root, `a/mod.rs` → `a`, `a/b.rs` → `a::b`.
fn module_path(rel: &[&str]) -> Option<Vec<String>> {
    let (last, dirs) = rel.split_last()?;
    let mut module: Vec<String> = dirs.iter().map(|d| (*d).to_owned()).collect();
    match *last {
        "lib.rs" | "main.rs" if dirs.is_empty() => {}
        "mod.rs" => {}
        name => module.push(name.strip_suffix(".rs")?.to_owned()),
    }
    Some(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway crate: `files` are created (empty) below its root.
    fn fake_crate(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        for f in files {
            let path = dir.path().join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        dir
    }

    fn loc(root: &Path, target: &str, module: &[&str]) -> Option<Location> {
        Some(Location {
            target_root: root.join(target),
            module: module.iter().map(|m| (*m).to_owned()).collect(),
        })
    }

    #[test]
    fn locates_library_binary_and_integration_test_files() {
        let lib = fake_crate(&["src/lib.rs", "src/main.rs", "tests/common/mod.rs"]);
        let r = lib.path();
        assert_eq!(locate(&r.join("src/lib.rs")), loc(r, "src/lib.rs", &[]));
        assert_eq!(
            locate(&r.join("src/a/mod.rs")),
            loc(r, "src/lib.rs", &["a"])
        );
        assert_eq!(
            locate(&r.join("src/a/b.rs")),
            loc(r, "src/lib.rs", &["a", "b"])
        );
        assert_eq!(locate(&r.join("tests/cli.rs")), loc(r, "tests/cli.rs", &[]));
        assert_eq!(locate(&r.join("tests/common/mod.rs")), None);
        assert_eq!(locate(&r.join("examples/demo.rs")), None);
        assert_eq!(locate(&r.join("build.rs")), None);

        let bin = fake_crate(&["src/main.rs", "tests/big/main.rs"]);
        let r = bin.path();
        assert_eq!(
            locate(&r.join("src/cmd.rs")),
            loc(r, "src/main.rs", &["cmd"])
        );
        assert_eq!(
            locate(&r.join("tests/big/part.rs")),
            loc(r, "tests/big/main.rs", &["part"])
        );
    }

    const SOURCE: &str = r#"
//! Verifies: FR-X-001 acceptance 9

/// Verifies: FR-AG03-001 acceptance 1
/// Verifies:   FR-AG03-001   acceptance 2
#[test]
fn top_level() {}

/// Verifies: FR-AG03-002 acceptance 1
fn helper() {}

#[cfg(test)]
mod tests {
    mod inner {
        /// Plain docs.
        /// Verifies: FR-AG03-003 acceptance 1
        #[tokio::test(flavor = "multi_thread")]
        async fn nested() {}

        #[test]
        fn untagged() {}
    }
}
"#;

    #[test]
    fn attaches_tags_to_tests_and_reports_stray_ones() {
        let krate = fake_crate(&["src/lib.rs"]);
        let file = krate.path().join("src/engine.rs");
        let mut scan = Scan::default();
        scan_file(&file, SOURCE, &mut scan).unwrap();

        let tests: Vec<_> = scan
            .tests
            .iter()
            .map(|t| {
                let tags: Vec<_> = t.tags.iter().map(|g| g.scenario.as_str()).collect();
                (t.name.as_str(), t.line, tags)
            })
            .collect();
        assert_eq!(
            tests,
            vec![
                (
                    "engine::top_level",
                    7,
                    vec!["FR-AG03-001 acceptance 1", "FR-AG03-001 acceptance 2"]
                ),
                (
                    "engine::tests::inner::nested",
                    18,
                    vec!["FR-AG03-003 acceptance 1"]
                ),
            ]
        );
        assert!(
            scan.tests
                .iter()
                .all(|t| t.target_root == krate.path().join("src/lib.rs"))
        );

        let stray: Vec<_> = scan
            .stray
            .iter()
            .map(|(t, _)| (t.scenario.as_str(), t.line))
            .collect();
        assert_eq!(
            stray,
            vec![
                ("FR-X-001 acceptance 9", 2),
                ("FR-AG03-002 acceptance 1", 9)
            ]
        );
    }

    #[test]
    fn tags_outside_a_target_are_stray() {
        let krate = fake_crate(&["src/lib.rs"]);
        let file = krate.path().join("examples/demo.rs");
        let mut scan = Scan::default();
        let source = "/// Verifies: FR-A-001 acceptance 1\n#[test]\nfn t() {}\n";
        scan_file(&file, source, &mut scan).unwrap();
        assert!(scan.tests.is_empty());
        assert_eq!(scan.stray.len(), 1);
    }
}
