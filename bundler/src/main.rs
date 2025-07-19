use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use quote::{format_ident, quote, ToTokens};
use syn::{parse_file, visit::Visit, File, Item, ItemMod, ItemUse, UseTree};

/// ------------------------------------------------------------
/// 1. ユーティリティ
/// ------------------------------------------------------------

/// `use_tree` を DFS して leaf パス (Vec<String>) を収集
fn collect_leaves(t: &UseTree,
                  prefix: &mut Vec<String>,
                  out: &mut Vec<Vec<String>>) {
    match t {
        UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            collect_leaves(&*p.tree, prefix, out);
            prefix.pop();
        }
        UseTree::Group(g) => {
            for item in &g.items {
                collect_leaves(item, prefix, out);
            }
        }
        UseTree::Name(n) => {
            let mut full = prefix.clone();
            full.push(n.ident.to_string());
            out.push(full);
        }
        UseTree::Rename(n) => {
            let mut full = prefix.clone();
            full.push(n.ident.to_string());
            out.push(full);
        }
        UseTree::Glob(_) => {}    // グロブは今回は無視
    }
}

/// `["adry_library","hash"]` → `<root>/hash.rs`
fn lib_file(root: &Path, segs: &[String]) -> PathBuf {
    let mut p = root.to_path_buf();
    for s in &segs[1..] {          // 先頭 (=crate 名) を捨てる
        p.push(s);
    }
    p.set_extension("rs");
    p
}

/// ------------------------------------------------------------
/// 2. モジュール木
/// ------------------------------------------------------------

#[derive(Default)]
struct Module {
    code: Option<String>,
    children: BTreeMap<String, Module>,
}

impl Module {
    fn insert(&mut self, segs: &[String], code: String) {
        match segs.split_first() {
            Some((head, rest)) if rest.is_empty() => {
                self.children.entry(head.clone()).or_default().code = Some(code);
            }
            Some((head, rest)) => {
                self.children
                    .entry(head.clone())
                    .or_default()
                    .insert(rest, code);
            }
            None => {}
        }
    }

    /// 子モジュールの宣言 `mod foo;` を除去して二重定義を防ぐ
    fn strip_decls(f: &File, child_names: &BTreeMap<String, Module>) -> Vec<Item> {
        f.items
            .iter()
            .filter(|it| {
                match it {
                    Item::Mod(ItemMod { content: None, ident, .. }) =>
                        !child_names.contains_key(&ident.to_string()),
                    _ => true,
                }
            })
            .cloned()
            .collect()
    }

    /// 再帰的に `pub mod …` を構築（ルートだけ公開しない）
    fn to_tokens(&self, name: Option<&str>) -> proc_macro2::TokenStream {
        let own_tokens = self.code.as_ref().map(|src| {
            let f: File = parse_file(src).expect("parse");
            let filtered = Self::strip_decls(&f, &self.children);
            quote! { #(#filtered)* }
        });

        let kids: Vec<_> = self
            .children
            .iter()
            .map(|(n, m)| m.to_tokens(Some(n)))
            .collect();

        match name {
            Some(n) => {
                let ident = format_ident!("{}", n);
                quote! {
                    pub mod #ident {
                        #own_tokens
                        #(#kids)*
                    }
                }
            }
            None => quote! {
                #own_tokens
                #(#kids)*
            },
        }
    }
}

/// ------------------------------------------------------------
/// 3. Main
/// ------------------------------------------------------------

fn main() -> Result<()> {
    //------------------------- 引数 -----------------------------
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: bundler <adry_library/src> <target.rs>");
        std::process::exit(1);
    }
    let lib_root = PathBuf::from(&args[1]);
    let target_rs = PathBuf::from(&args[2]);

    //----------------------- 対象ファイル ------------------------
    let target_src = fs::read_to_string(&target_rs)
        .with_context(|| format!("read {:?}", target_rs))?;
    let target_ast: File = parse_file(&target_src)?;

    //------------------  use adry_library::* 収集 ----------------
    struct Collector<'a> {
        leaves: Vec<Vec<String>>,
        root_ident: &'a str,
    }
    impl<'ast, 'a> Visit<'ast> for Collector<'a> {
        fn visit_item_use(&mut self, i: &'ast ItemUse) {
            if let UseTree::Path(p) = &i.tree {
                if p.ident == self.root_ident {
                    let mut prefix = vec![p.ident.to_string()];
                    collect_leaves(&*p.tree, &mut prefix, &mut self.leaves);
                }
            }
            syn::visit::visit_item_use(self, i);
        }
    }

    let mut coll = Collector {
        leaves: Vec::new(),
        root_ident: "adry_library",
    };
    coll.visit_file(&target_ast);

    if coll.leaves.is_empty() {
        // adry_library を使っていない → そのまま出力
        print!("{target_src}");
        return Ok(());
    }

    //---------------- モジュール集合 (親パス) --------------------
    let mut parents = BTreeSet::<Vec<String>>::new();
    for leaf in coll.leaves {
        if leaf.len() >= 2 {
            parents.insert(leaf[..leaf.len() - 1].to_vec());
        }
    }

    //------------------ ライブラリ読み込み -----------------------
    let mut root_mod = Module::default();
    for segs in &parents {
        let fp = lib_file(&lib_root, segs);
        let code = fs::read_to_string(&fp)
            .with_context(|| format!("read {:?}", fp))?;
        root_mod.insert(segs, code);
    }

    //------------------ 生成 → prettyplease ---------------------
    let lib_ts = root_mod.to_tokens(None);
    let pretty = match syn::parse2::<File>(lib_ts.clone()) {
        Ok(ast) => prettyplease::unparse(&ast),
        Err(e) => {
            eprintln!("prettyplease failed (falling back): {e}");
            lib_ts.to_string()
        }
    };

    //---------------------- 出力 -------------------------------
    println!("{target_src}\n\n// ===== bundled library =====\n\n{pretty}");
    Ok(())
}
