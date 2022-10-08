use std::{borrow::Cow, collections::HashMap, io::Read, ops::Range, path::PathBuf};

use markdown_gen::markdown;
use proc_macro2::{LineColumn, Span};
use syn::spanned::Spanned;

/// Copies doc comments from C sources into Rust sources.
///
/// Any Rust functions/structs/enums annotated with `#[doc(alias = "func")]`
/// will receive doc comments from the corresponding C function.
#[derive(clap::Parser, Debug, Default)]
struct Args {
    /// Rewrite Rust files in place.
    #[clap(short, long)]
    in_place: bool,
    /// Backup files before writing. Must be used with -i.
    #[clap(short, long)]
    backup: bool,
    /// List of C sources to pull doc comments from.
    #[clap(short, long)]
    c_srcs: Vec<PathBuf>,
    /// List of Rust sources to parse and insert doc comments into.
    rust_srcs: Vec<PathBuf>,
}

mod keywords {
    syn::custom_keyword!(alias);
}

struct DocComment(syn::LitStr);

impl syn::parse::Parse for DocComment {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        input.parse::<syn::Token![=]>()?;
        let s = input.parse::<syn::LitStr>()?;
        input.parse::<syn::parse::Nothing>()?;
        Ok(Self(s))
    }
}

impl DocComment {
    fn find(attrs: &[syn::Attribute]) -> Option<syn::LitStr> {
        attrs.iter().find_map(|a| {
            a.path
                .is_ident("doc")
                .then(|| syn::parse2::<Self>(a.tokens.clone()).ok().map(|d| d.0))
                .flatten()
        })
    }
}

struct DocAlias(String);

impl syn::parse::Parse for DocAlias {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let content;
        syn::parenthesized!(content in input);
        input.parse::<syn::parse::Nothing>()?;
        content.parse::<keywords::alias>()?;
        content.parse::<syn::Token![=]>()?;
        let s = content.parse::<syn::LitStr>()?;
        if content.peek(syn::Token![,]) {
            content.parse::<syn::Token![,]>()?;
        }
        content.parse::<syn::parse::Nothing>()?;
        Ok(Self(s.value()))
    }
}

impl DocAlias {
    fn find(attrs: &[syn::Attribute]) -> Option<String> {
        attrs.iter().find_map(|a| {
            a.path
                .is_ident("doc")
                .then(|| syn::parse2::<Self>(a.tokens.clone()).ok().map(|d| d.0))
                .flatten()
        })
    }
}

struct Source<'s> {
    full: &'s str,
    lines: Vec<&'s str>,
}

impl<'s> Source<'s> {
    fn position(&self, pos: LineColumn) -> Option<usize> {
        self.lines.get(pos.line - 1).and_then(|l| {
            let index = l
                .char_indices()
                .nth(pos.column)
                .map(|i| i.0)
                .unwrap_or_else(|| l.len());
            Some(l.get(index..index)?.as_ptr() as usize - self.full.as_ptr() as usize)
        })
    }
    fn range_for(&self, span: Span) -> Option<Range<usize>> {
        Some(self.position(span.start())?..self.position(span.end())?)
    }
}

struct DocVisitor<'s> {
    source: Source<'s>,
    doc_locations: HashMap<String, Vec<(usize, Range<usize>)>>,
}

impl<'s> DocVisitor<'s> {
    fn try_replace_docs(&mut self, span: Span, attrs: &[syn::Attribute]) {
        if let Some(alias) = DocAlias::find(attrs) {
            let locations = self.doc_locations.entry(alias).or_default();
            let mut has_docs = false;
            if let Some(doc) = DocComment::find(attrs) {
                if let Some(range) = self.source.range_for(doc.span()) {
                    locations.push((doc.span().start().column, range));
                    has_docs = true;
                }
            }
            if !has_docs {
                if let Some(pos) = self.source.position(span.start()) {
                    locations.push((span.start().column, pos..pos));
                }
            }
        }
    }
}

impl<'ast, 's> syn::visit::Visit<'ast> for DocVisitor<'s> {
    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.try_replace_docs(i.span(), &i.attrs);
        syn::visit::visit_item_fn(self, i);
    }
    fn visit_impl_item_method(&mut self, i: &'ast syn::ImplItemMethod) {
        self.try_replace_docs(i.span(), &i.attrs);
        syn::visit::visit_impl_item_method(self, i);
    }
    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        self.try_replace_docs(i.span(), &i.attrs);
        syn::visit::visit_item_struct(self, i);
    }
    fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
        self.try_replace_docs(i.span(), &i.attrs);
        syn::visit::visit_item_enum(self, i);
    }
    fn visit_variant(&mut self, i: &'ast syn::Variant) {
        self.try_replace_docs(i.span(), &i.attrs);
        syn::visit::visit_variant(self, i);
    }
    fn visit_item_const(&mut self, i: &'ast syn::ItemConst) {
        self.try_replace_docs(i.span(), &i.attrs);
        syn::visit::visit_item_const(self, i);
    }
    fn visit_impl_item_const(&mut self, i: &'ast syn::ImplItemConst) {
        self.try_replace_docs(i.span(), &i.attrs);
        syn::visit::visit_impl_item_const(self, i);
    }
}

struct RustFile {
    path: PathBuf,
    source: String,
    doc_locations: HashMap<String, Vec<(usize, Range<usize>)>>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = <Args as clap::Parser>::parse();
    let mut files = Vec::new();
    for src in args.rust_srcs {
        for path in glob::glob(src.to_string_lossy().as_ref())? {
            let path = path?;
            if !path.is_file() {
                continue;
            }
            let mut file = std::fs::File::open(&path)?;
            let mut source = String::new();
            file.read_to_string(&mut source)?;
            let ast = syn::parse_file(&source)?;
            let doc_locations = {
                let mut visitor = DocVisitor {
                    source: Source {
                        full: source.as_str(),
                        lines: source.lines().collect(),
                    },
                    doc_locations: HashMap::new(),
                };
                syn::visit::Visit::visit_file(&mut visitor, &ast);
                visitor.doc_locations
            };
            files.push(RustFile {
                path,
                source,
                doc_locations,
            });
        }
    }
    let clang = clang::Clang::new().unwrap();
    let index = clang::Index::new(&clang, true, false);
    let mut c_docs = files
        .iter()
        .flat_map(|f| f.doc_locations.keys().cloned().map(|s| (s, String::new())))
        .collect::<HashMap<_, _>>();
    for src in args.c_srcs {
        for path in glob::glob(src.to_string_lossy().as_ref())? {
            let path = path?;
            if !path.is_file() {
                continue;
            }
            let parser = index.parser(path);
            let tu = parser.parse()?;
            let entity = tu.get_entity();
            let mut res = Ok(());
            entity.visit_children(|e, _| {
                use clang::EntityKind;
                let k = e.get_kind();
                if k == EntityKind::FunctionDecl
                    || k == EntityKind::StructDecl
                    || k == EntityKind::TypedefDecl
                    || k == EntityKind::EnumDecl
                    || k == EntityKind::EnumConstantDecl
                {
                    if let (Some(name), Some(comment)) = (e.get_name(), e.get_parsed_comment()) {
                        if let Some(doc) = c_docs.get_mut(&name) {
                            if doc.is_empty() {
                                match xml_to_markdown(&comment.as_xml()) {
                                    Ok(d) => *doc = d,
                                    Err(e) => {
                                        res = Err(e);
                                        return clang::EntityVisitResult::Break;
                                    }
                                }
                            }
                        }
                    }
                }
                clang::EntityVisitResult::Recurse
            });
            res?;
        }
    }
    for mut file in files {
        let mut changed = false;
        let orig = (args.in_place && args.backup).then(|| file.source.clone());
        let mut replacements = Vec::new();
        for (ident, ranges) in file.doc_locations {
            if let Some(doc) = c_docs.get(&ident) {
                if !doc.is_empty() {
                    changed = true;
                    for (column, range) in ranges {
                        let doc = if column > 0 {
                            let mut doc = doc
                                .lines()
                                .enumerate()
                                .map(|(i, line)| {
                                    let mut line = line.to_owned();
                                    if i > 0 {
                                        line.insert_str(0, &" ".repeat(column));
                                    }
                                    line
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            doc.push('\n');
                            doc.push_str(&" ".repeat(column));
                            doc.into()
                        } else {
                            Cow::Borrowed(doc.as_str())
                        };
                        replacements.push((doc, range));
                    }
                }
            }
        }
        replacements.sort_by_key(|(_, range)| range.start);
        for (doc, range) in replacements.into_iter().rev() {
            file.source.replace_range(range, doc.as_ref());
        }
        if !args.in_place {
            println!("{}:\n{}", file.path.display(), file.source);
        } else if changed {
            if let Some(orig) = orig {
                std::fs::write(file.path.with_extension("bk"), orig.into_bytes())?;
            }
            std::fs::write(file.path, file.source.into_bytes())?;
        }
    }
    Ok(())
}

fn get_paragraphs<'n>(
    node: roxmltree::Node<'n, '_>,
) -> impl Iterator<Item = markdown::Paragraph<'n>> + 'n {
    use markdown::AsMarkdown;
    node.children()
        .filter(|n| n.has_tag_name("Para"))
        .map(|para| {
            para.children().fold("".paragraph(), |item, c| {
                if c.is_text() {
                    return item.append(c.text().unwrap());
                } else if c.is_element() {
                    if let Some(t) = c.text() {
                        if c.has_tag_name("emphasized") {
                            return item.append(t.code());
                        } else {
                            return item.append(t);
                        }
                    } else {
                        return c.descendants().fold(item, |item, cc| {
                            if cc.is_text() {
                                item.append(c.text().unwrap())
                            } else {
                                item
                            }
                        });
                    }
                }
                item
            })
        })
}

#[inline]
fn write_paragraphs(md: &mut markdown::Markdown<Vec<u8>>, node: roxmltree::Node) {
    for para in get_paragraphs(node) {
        md.write(para).unwrap();
    }
}

fn xml_to_markdown(xml: &str) -> Result<String, roxmltree::Error> {
    use markdown_gen::markdown::AsMarkdown;
    /*
    xmltree::Element::parse(xml.as_bytes())
        .unwrap()
        .write_with_config(
            std::io::stderr(),
            xmltree::EmitterConfig::new().perform_indent(true),
        )
        .unwrap();
    eprintln!("");
    */
    let document = roxmltree::Document::parse(xml)?;
    let mut md = markdown::Markdown::new(Vec::new());

    let root = document.root_element();
    if let Some(abs) = root.children().find(|n| n.has_tag_name("Abstract")) {
        write_paragraphs(&mut md, abs);
    }
    for disc in root.children().filter(|n| n.has_tag_name("Discussion")) {
        write_paragraphs(&mut md, disc);
    }
    if let Some(params) = root.children().find(|n| n.has_tag_name("Parameters")) {
        let mut has_params = false;
        let list = params
            .children()
            .filter(|n| n.has_tag_name("Parameter"))
            .fold(markdown::List::new(false), |list, param| {
                if let Some(name) = param
                    .children()
                    .find(|n| n.has_tag_name("Name"))
                    .and_then(|n| n.text())
                {
                    has_params = true;
                    let item = name.code().paragraph();
                    let item = param.children().fold(item, |item, n| {
                        if n.has_tag_name("Discussion") {
                            return get_paragraphs(n)
                                .fold(item, |item, para| item.append("\n\n ").append(para));
                        }
                        item
                    });
                    return list.item(item);
                }
                list
            });
        if has_params {
            md.write("Parameters".heading(1)).unwrap();
            md.write(list.paragraph().append("\n")).unwrap();
        }
    }
    if let Some(returns) = root.children().find(|n| n.has_tag_name("ResultDiscussion")) {
        md.write("Returns").unwrap();
        write_paragraphs(&mut md, returns);
    }
    let inner = md.into_inner();
    let src = String::from_utf8_lossy(&inner);
    let src = src
        .lines()
        .map(|line| {
            let mut line = line.to_owned();
            line.insert_str(0, "/// ");
            line
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(src)
}
