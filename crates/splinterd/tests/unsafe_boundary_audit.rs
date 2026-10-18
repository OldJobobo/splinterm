//! Source audit of the `splinterd` package's one audited unsafe exception.
//!
//! The workspace forbids `unsafe_code`, which no package can relax locally,
//! so this package copies the workspace lint table with `deny`. This audit
//! proves the copy is exact and that the only unsafe construct and the only
//! `unsafe_code` waiver anywhere in the package's sources are the post-exec
//! acquisition in `src/candidate_acquire.rs`.

use std::{fs, path::Path};

use syn::{
    Attribute, ExprUnsafe, ImplItemFn, ItemFn, ItemForeignMod, ItemImpl, ItemStatic, ItemTrait,
    Macro, StaticMutability, TraitItemFn,
    visit::{self, Visit},
};

#[derive(Debug, Eq, PartialEq)]
struct Finding {
    file: String,
    function: Option<String>,
    kind: &'static str,
}

#[derive(Default)]
struct UnsafeAudit {
    file: String,
    function: Option<String>,
    constructs: Vec<Finding>,
    waivers: Vec<Finding>,
}

impl UnsafeAudit {
    fn finding(&self, kind: &'static str) -> Finding {
        Finding {
            file: self.file.clone(),
            function: self.function.clone(),
            kind,
        }
    }
}

impl<'ast> Visit<'ast> for UnsafeAudit {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let previous = self.function.replace(node.sig.ident.to_string());
        if node.sig.unsafety.is_some() {
            self.constructs.push(self.finding("unsafe function"));
        }
        visit::visit_item_fn(self, node);
        self.function = previous;
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        let previous = self.function.replace(node.sig.ident.to_string());
        if node.sig.unsafety.is_some() {
            self.constructs.push(self.finding("unsafe impl function"));
        }
        visit::visit_impl_item_fn(self, node);
        self.function = previous;
    }

    fn visit_trait_item_fn(&mut self, node: &'ast TraitItemFn) {
        let previous = self.function.replace(node.sig.ident.to_string());
        if node.sig.unsafety.is_some() {
            self.constructs.push(self.finding("unsafe trait function"));
        }
        visit::visit_trait_item_fn(self, node);
        self.function = previous;
    }

    fn visit_expr_unsafe(&mut self, node: &'ast ExprUnsafe) {
        self.constructs.push(self.finding("unsafe block"));
        visit::visit_expr_unsafe(self, node);
    }

    fn visit_item_trait(&mut self, node: &'ast ItemTrait) {
        if node.unsafety.is_some() {
            self.constructs.push(self.finding("unsafe trait"));
        }
        visit::visit_item_trait(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        if node.unsafety.is_some() {
            self.constructs.push(self.finding("unsafe impl"));
        }
        visit::visit_item_impl(self, node);
    }

    fn visit_item_foreign_mod(&mut self, node: &'ast ItemForeignMod) {
        if node.unsafety.is_some() {
            self.constructs.push(self.finding("unsafe extern block"));
        }
        visit::visit_item_foreign_mod(self, node);
    }

    fn visit_item_static(&mut self, node: &'ast ItemStatic) {
        if matches!(node.mutability, StaticMutability::Mut(_)) {
            self.constructs.push(self.finding("mutable static"));
        }
        visit::visit_item_static(self, node);
    }

    fn visit_attribute(&mut self, node: &'ast Attribute) {
        let direct_allow = node.path().is_ident("allow");
        let direct_expect = node.path().is_ident("expect");
        let direct_unsafe = node.path().is_ident("unsafe");
        let nested_allow = meta_contains_ident(&node.meta, "allow");
        let nested_expect = meta_contains_ident(&node.meta, "expect");
        let mentions_unsafe_code = meta_contains_ident(&node.meta, "unsafe_code");
        let nested_unsafe = meta_contains_ident(&node.meta, "unsafe");

        if direct_unsafe {
            self.constructs.push(self.finding("unsafe attribute"));
        } else if nested_unsafe {
            self.constructs
                .push(self.finding("nested unsafe attribute"));
        }
        if mentions_unsafe_code && (direct_allow || direct_expect) {
            self.waivers.push(self.finding(if direct_allow {
                "allow unsafe_code"
            } else {
                "expect unsafe_code"
            }));
        } else if mentions_unsafe_code && (nested_allow || nested_expect) {
            self.waivers.push(self.finding("nested unsafe_code waiver"));
        } else if mentions_unsafe_code {
            // `warn`, `deny`, or `forbid` changes nothing an audit must allow,
            // but any other lint-level spelling is a waiver to review.
            let level = node.path().get_ident().map(ToString::to_string);
            if !matches!(level.as_deref(), Some("deny" | "forbid")) {
                self.waivers.push(self.finding("unsafe_code lint level"));
            }
        }
        visit::visit_attribute(self, node);
    }

    fn visit_macro(&mut self, node: &'ast Macro) {
        if tokens_contain_ident(node.tokens.clone(), "unsafe") {
            self.constructs.push(self.finding("macro unsafe token"));
        }
        if tokens_contain_ident(node.tokens.clone(), "unsafe_code") {
            self.waivers.push(self.finding("macro unsafe_code token"));
        }
        visit::visit_macro(self, node);
    }
}

fn meta_contains_ident(meta: &syn::Meta, expected: &str) -> bool {
    match meta {
        syn::Meta::Path(path) => path.is_ident(expected),
        syn::Meta::List(list) => tokens_contain_ident(list.tokens.clone(), expected),
        syn::Meta::NameValue(value) => value.path.is_ident(expected),
    }
}

fn tokens_contain_ident(tokens: proc_macro2::TokenStream, expected: &str) -> bool {
    tokens.into_iter().any(|token| match token {
        proc_macro2::TokenTree::Ident(ident) => ident == expected,
        proc_macro2::TokenTree::Group(group) => tokens_contain_ident(group.stream(), expected),
        proc_macro2::TokenTree::Punct(_) | proc_macro2::TokenTree::Literal(_) => false,
    })
}

fn audit_source(file: &str, source: &str) -> UnsafeAudit {
    let syntax = syn::parse_file(source)
        .unwrap_or_else(|error| panic!("cannot parse synthetic {file}: {error}"));
    let mut audit = UnsafeAudit {
        file: file.to_owned(),
        ..UnsafeAudit::default()
    };
    audit.visit_file(&syntax);
    audit
}

fn audit_file(root: &Path, path: &Path, audit: &mut UnsafeAudit) {
    audit.file = path
        .strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    audit.function = None;
    let source = fs::read_to_string(path).unwrap();
    let syntax = syn::parse_file(&source)
        .unwrap_or_else(|error| panic!("cannot parse {}: {error}", path.display()));
    audit.visit_file(&syntax);
}

fn audit_sources(root: &Path, directory: &Path, audit: &mut UnsafeAudit) {
    let mut entries = fs::read_dir(directory)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            audit_sources(root, &path, audit);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            audit_file(root, &path, audit);
        }
    }
}

fn has_exact_lint_policy(cargo: &str) -> bool {
    let Ok(document) = toml::from_str::<toml::Value>(cargo) else {
        return false;
    };
    let Some(lints) = document.get("lints").and_then(toml::Value::as_table) else {
        return false;
    };
    let Some(rust) = lints.get("rust").and_then(toml::Value::as_table) else {
        return false;
    };
    let Some(clippy) = lints.get("clippy").and_then(toml::Value::as_table) else {
        return false;
    };
    lints.len() == 2
        && rust.len() == 1
        && rust.get("unsafe_code").and_then(toml::Value::as_str) == Some("deny")
        && clippy.len() == 2
        && clippy.get("all").and_then(toml::Value::as_str) == Some("warn")
        && clippy.get("pedantic").and_then(toml::Value::as_str) == Some("warn")
}

/// The package copy must track the workspace table it replaces, apart from
/// the one `forbid` to `deny` relaxation.
fn workspace_lints_match_package_copy(workspace: &str) -> bool {
    let Ok(document) = toml::from_str::<toml::Value>(workspace) else {
        return false;
    };
    let Some(lints) = document
        .get("workspace")
        .and_then(|workspace| workspace.get("lints"))
        .and_then(toml::Value::as_table)
    else {
        return false;
    };
    let rust = lints.get("rust").and_then(toml::Value::as_table);
    let clippy = lints.get("clippy").and_then(toml::Value::as_table);
    lints.len() == 2
        && rust.is_some_and(|rust| {
            rust.len() == 1
                && rust.get("unsafe_code").and_then(toml::Value::as_str) == Some("forbid")
        })
        && clippy.is_some_and(|clippy| {
            clippy.len() == 2
                && clippy.get("all").and_then(toml::Value::as_str) == Some("warn")
                && clippy.get("pedantic").and_then(toml::Value::as_str) == Some("warn")
        })
}

#[test]
fn lint_policy_must_be_in_the_exact_toml_sections() {
    assert!(!has_exact_lint_policy(
        r#"
            [lints.rust]
            [lints.clippy]
            unsafe_code = "deny"
            all = "warn"
            pedantic = "warn"
        "#
    ));
    assert!(!has_exact_lint_policy(
        r#"
            [lints]
            workspace = true
            [lints.rust]
            unsafe_code = "deny"
            [lints.clippy]
            all = "warn"
            pedantic = "warn"
        "#
    ));
    assert!(!has_exact_lint_policy(
        r#"
            [lints.rust]
            unsafe_code = "allow"
            [lints.clippy]
            all = "warn"
            pedantic = "warn"
        "#
    ));
}

#[test]
fn nested_attribute_waivers_and_unsafe_attributes_are_detected() {
    let audit = audit_source(
        "synthetic.rs",
        r"
            #[cfg_attr(any(), allow(unsafe_code))]
            fn hidden_allow() {}
            #[cfg_attr(any(), expect(unsafe_code))]
            fn hidden_expect() {}
            #[cfg_attr(any(), unsafe(no_mangle))]
            fn hidden_unsafe_attribute() {}
            #[warn(unsafe_code)]
            fn weakened() {}
            #[deny(unsafe_code)]
            fn strengthened() {}
        ",
    );
    assert_eq!(
        audit.waivers,
        [
            Finding {
                file: "synthetic.rs".to_owned(),
                function: Some("hidden_allow".to_owned()),
                kind: "nested unsafe_code waiver",
            },
            Finding {
                file: "synthetic.rs".to_owned(),
                function: Some("hidden_expect".to_owned()),
                kind: "nested unsafe_code waiver",
            },
            Finding {
                file: "synthetic.rs".to_owned(),
                function: Some("weakened".to_owned()),
                kind: "unsafe_code lint level",
            },
        ]
    );
    assert_eq!(
        audit.constructs,
        [Finding {
            file: "synthetic.rs".to_owned(),
            function: Some("hidden_unsafe_attribute".to_owned()),
            kind: "nested unsafe attribute",
        }]
    );
}

#[test]
fn splinterd_keeps_one_exact_unsafe_bootstrap_invocation_and_policy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(has_exact_lint_policy(&cargo));
    let workspace = fs::read_to_string(root.join("../../Cargo.toml")).unwrap();
    assert!(workspace_lints_match_package_copy(&workspace));

    let mut audit = UnsafeAudit::default();
    audit_file(root, &root.join("build.rs"), &mut audit);
    for directory in ["src", "tests", "examples"] {
        audit_sources(root, &root.join(directory), &mut audit);
    }
    let expected_construct = Finding {
        file: "src/candidate_acquire.rs".to_owned(),
        function: Some("acquire_before_tokio".to_owned()),
        kind: "unsafe block",
    };
    let expected_waiver = Finding {
        file: "src/candidate_acquire.rs".to_owned(),
        function: Some("acquire_before_tokio".to_owned()),
        kind: "allow unsafe_code",
    };
    assert_eq!(audit.constructs, [expected_construct]);
    assert_eq!(audit.waivers, [expected_waiver]);
}
