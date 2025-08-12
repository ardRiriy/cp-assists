use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use quote::{format_ident, quote};
use syn::{parse_file, visit::Visit, File, Item, ItemMod, ItemUse, UseTree};

///------------------------------------------------------------
/// 1. ユーティリティ
///------------------------------------------------------------

fn collect_leaves(t: &UseTree,
                  prefix: &mut Vec<String>,
                  out: &mut Vec<Vec<String>>) {
    match t {
        UseTree::Path(p) => { prefix.push(p.ident.to_string());
            collect_leaves(&*p.tree, prefix, out);
            prefix.pop(); }
        UseTree::Group(g) => {
            for item in &g.items { collect_leaves(item, prefix, out); }
        }
        UseTree::Name(n) => {
            let mut full = prefix.clone(); full.push(n.ident.to_string()); out.push(full);
        }
        UseTree::Rename(n) => {
            let mut full = prefix.clone(); full.push(n.ident.to_string()); out.push(full);
        }
        UseTree::Glob(_) => {} // グロブは無視
    }
}

/// ["adry_library","hash","fenwick"] → <root>/hash/fenwick.rs
fn lib_file(root: &Path, segs: &[String]) -> PathBuf {
    let mut p = root.to_path_buf();
    for s in &segs[1..] { 
        p.push(s);
    }
    let mut cand = p.clone();
    cand.set_extension("rs");
    if cand.is_file() {
        return cand;
    }
    p
}

///------------------------------------------------------------
/// 2. モジュール木
///------------------------------------------------------------

#[derive(Default)]
struct Module {
    code: Option<String>,
    children: BTreeMap<String, Module>,
}

impl Module {
    fn insert(&mut self, segs: &[String], code: String) {
        match segs.split_first() {
            Some((head, rest)) if rest.is_empty() => {
                self.children.entry(head.clone()).or_default().code = Some(code)
            }
            Some((head, rest)) => self.children.entry(head.clone()).or_default().insert(rest, code),
            None => {}
        }
    }
    fn strip_decls(f: &File, child_names: &BTreeMap<String, Module>) -> Vec<Item> {
        f.items.iter().filter(|it| match it {
            Item::Mod(ItemMod { content: None, ident, .. })
                => !child_names.contains_key(&ident.to_string()),
            _ => true
        }).cloned().collect()
    }
    fn to_tokens(&self, name: Option<&str>) -> proc_macro2::TokenStream {
        let own_tokens = self.code.as_ref().map(|src| {
            let f: File = parse_file(src).expect("parse"); // ライブラリ内はほぼパース通る前提
            let filtered = Self::strip_decls(&f, &self.children);
            quote! { #(#filtered)* }
        });
        let kids: Vec<_> = self.children.iter().map(|(n, m)| m.to_tokens(Some(n))).collect();
        match name {
            Some(n) => { let ident = format_ident!("{n}");
                quote! { pub mod #ident { #own_tokens #(#kids)* } } }
            None    => quote! { #own_tokens #(#kids)* },
        }
    }
}

///------------------------------------------------------------
/// 3. 内部 use 探索 (crate:: / super::)
///------------------------------------------------------------

fn internal_deps(ast: &File, cur_path: &[String]) -> Vec<Vec<String>> {
    struct V<'a> { out: &'a mut Vec<Vec<String>>, cur: &'a [String] }
    impl<'ast,'a> Visit<'ast> for V<'a> {
        fn visit_item_use(&mut self, i: &'ast ItemUse) {
            match &i.tree {
                UseTree::Path(p) if p.ident == "crate" => {
                    let mut segs = vec!["library".into()];
                    collect_leaves(&*p.tree, &mut segs, self.out);
                    if segs.len() > 1 {
                        segs.pop();
                    }
                }
                UseTree::Path(p) if p.ident == "super" && !self.cur.is_empty() => {
                    let mut base = self.cur[..self.cur.len()-1].to_vec(); // 1段上へ
                    collect_leaves(&*p.tree, &mut base, self.out);
                    if base.len() > 1 {
                        base.pop();
                    }
                }
                _ => {}
            }
            syn::visit::visit_item_use(self, i);
        }
    }
    let mut v = Vec::new();
    V { out: &mut v, cur: cur_path }.visit_file(ast);
    v
}

///------------------------------------------------------------
/// 4. Main
///------------------------------------------------------------

fn main() -> Result<()> {
    // ------------------------ 引数 ---------------------------
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: bundler <adry_library/src> <target.rs>");
        std::process::exit(1);
    }
    let lib_root = PathBuf::from(&args[1]);
    let target_rs = PathBuf::from(&args[2]);

    // --------------------- ターゲット読み ---------------------
    let target_src = fs::read_to_string(&target_rs)
        .with_context(|| format!("read {:?}", target_rs))?;
    let target_ast: File = parse_file(&target_src)?;

    // ----------- use library::… の leaf を集める ----------
    struct Collector<'a> { out: Vec<Vec<String>>, root: &'a str }
    impl<'ast,'a> Visit<'ast> for Collector<'a> {
        fn visit_item_use(&mut self, i: &'ast ItemUse) {
            if let UseTree::Path(p) = &i.tree {
                if p.ident == self.root {
                    let mut pre = vec![p.ident.to_string()];
                    collect_leaves(&*p.tree, &mut pre, &mut self.out);
                }
            }
            syn::visit::visit_item_use(self, i);
        }
    }
    let mut c = Collector { out: Vec::new(), root: "library" };
    c.visit_file(&target_ast);

    if c.out.is_empty() {
        print!("{target_src}");
        return Ok(())
    }

    // -------------- 再帰的にライブラリを束ねる ------------------
    let mut root_mod  = Module::default();
    let mut visited   = BTreeSet::<Vec<String>>::new();
    let mut queue: Vec<Vec<String>> = c
        .out
        .into_iter()
        .filter_map(|mut path| {
            if path.len() > 1 { path.pop(); Some(path) } else { None }
        })
        .collect();

    while let Some(path) = queue.pop() {
        if !visited.insert(path.clone()) { continue; }

        let fp = lib_file(&lib_root, &path);
        if let Ok(code) = fs::read_to_string(&fp)
            .with_context(|| format!("read {:?}", fp)) 
        {
            root_mod.insert(&path, code.clone());

            let ast: File = parse_file(&code)?;
            for dep in internal_deps(&ast, &path) {
                let mut dep = dep.clone();
                dep.pop();
                if !visited.contains(&dep) { queue.push(dep); }
            }
        } else {
            continue;
        }
    }

    // --------------------- prettyprint ------------------------
    let lib_ts = root_mod.to_tokens(None);
    let lib_pretty = match syn::parse2::<File>(lib_ts.clone()) {
        Ok(ast) => prettyplease::unparse(&ast),
        Err(e)  => { eprintln!("prettyplease failed: {e}"); lib_ts.to_string() }
    };

    // lib_prettyのuse crate::hogeをcrate::library::hogeに変換
    let lib_pretty = lib_pretty.replace("use crate::", "use crate::library::");
        
    // ------------------------ 出力 ---------------------------
    println!("{target_src}\n\n// ===== bundled library =====\n\n{lib_pretty}");
    Ok(())
}
