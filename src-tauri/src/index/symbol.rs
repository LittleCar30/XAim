//! 符号提取：用 tree-sitter 解析源码，按语法节点抽取函数/类/方法等符号，
//! 供「按符号名检索」使用，比纯关键词匹配更准。
//!
//! - 行号从 1 开始，`start_line`/`end_line` 为符号起止行（闭区间）；
//! - 语言不支持（如 text/未知）返回空 `Vec`；源码含语法错误时尽力提取
//!   其余合法声明，均不 panic；
//! - `lang` 取值与 [`crate::index::lang::detect_language`] 的返回一致。
//!
//! 各语言抽取的节点（kind 归入 function/class/method/interface/type/variable）：
//!
//! | 语言        | 语法节点                                                                  |
//! |-------------|---------------------------------------------------------------------------|
//! | typescript  | function_declaration, class_declaration, method_definition,               |
//! |             | interface_declaration, type_alias_declaration, lexical/variable_declaration |
//! | javascript  | function_declaration, class_declaration, method_definition,               |
//! |             | lexical/variable_declaration                                              |
//! | python      | function_definition（类体中归为 method）, class_definition                |
//! | rust        | function_item（impl 中归为 method）, struct_item（归为 class）, impl_item |
//! | go          | function_declaration, method_declaration, type_spec（归为 type）          |
//! | java        | method_declaration, class_declaration, interface_declaration              |

use std::path::Path;

use tree_sitter::{Node, Parser, TreeCursor};

use crate::errors::{Result, XaimError};

/// 一个可跳转/可检索的代码符号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// function / class / method / interface / type / variable。
    pub kind: String,
    pub file: String,
    /// 起始行，从 1 开始。
    pub start_line: u32,
    /// 结束行，从 1 开始（含）。
    pub end_line: u32,
}

/// 从 `content` 中提取 `lang` 语言的符号；`path` 仅用于回填符号的 file 字段。
pub fn extract_symbols(path: &Path, lang: &str, content: &str) -> Result<Vec<Symbol>> {
    let Some(grammar) = grammar_for(lang) else {
        return Ok(Vec::new()); // 不支持的语言（text 等）：无符号可提
    };
    let mut parser = Parser::new();
    parser
        .set_language(&grammar)
        .map_err(|err| XaimError::Unknown(format!("tree-sitter 语言 `{lang}` 加载失败: {err}")))?;
    let Some(tree) = parser.parse(content, None) else {
        return Ok(Vec::new()); // 未产出语法树：按无符号处理，不报错
    };

    let file = path.to_string_lossy().into_owned();
    let mut symbols = Vec::new();
    let mut cursor = tree.walk();
    visit(&mut cursor, lang, content, &file, &mut symbols);
    Ok(symbols)
}

/// lang 标识 → tree-sitter grammar。
///
/// `.ts` 与 `.tsx` 统一用 TSX 语法解析：TSX 是 TS 的超集（仅旧式 `<T>expr`
/// 断言不支持），声明节点两种语法完全一致，可同时覆盖两类文件。
fn grammar_for(lang: &str) -> Option<tree_sitter::Language> {
    match lang {
        "typescript" => Some(tree_sitter_typescript::language_tsx()),
        "javascript" => Some(tree_sitter_javascript::language()),
        "python" => Some(tree_sitter_python::language()),
        "rust" => Some(tree_sitter_rust::language()),
        "go" => Some(tree_sitter_go::language()),
        "java" => Some(tree_sitter_java::language()),
        _ => None,
    }
}

/// 先序遍历语法树，命中「可产生符号的节点类型」时收集符号。
fn visit(cursor: &mut TreeCursor, lang: &str, content: &str, file: &str, out: &mut Vec<Symbol>) {
    if let Some(symbols) = symbols_at(&cursor.node(), lang, content, file) {
        out.extend(symbols);
    }
    if cursor.goto_first_child() {
        loop {
            visit(cursor, lang, content, file, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// 单个语法节点 → 0..n 个符号；不关心的节点类型返回 `None`。
fn symbols_at(node: &Node, lang: &str, content: &str, file: &str) -> Option<Vec<Symbol>> {
    let field = |name: &str| field_text(node, name, content);
    match (lang, node.kind()) {
        // TypeScript / JavaScript 共享的声明节点（export 语句内的声明同样命中）。
        ("typescript" | "javascript", "function_declaration") => {
            single(node, "function", field("name"), file)
        }
        ("typescript" | "javascript", "class_declaration") => {
            single(node, "class", field("name"), file)
        }
        ("typescript" | "javascript", "method_definition") => {
            single(node, "method", field("name"), file)
        }
        ("typescript", "interface_declaration") => single(node, "interface", field("name"), file),
        ("typescript", "type_alias_declaration") => single(node, "type", field("name"), file),
        ("typescript" | "javascript", "lexical_declaration" | "variable_declaration") => {
            // const a = 1, b = 2 → 每个声明符一个符号；只取标识符形式的名字，
            // 解构模式（const {a, b} = ...）没有可检索的单一名字。
            let mut child_cursor = node.walk();
            let symbols = node
                .children(&mut child_cursor)
                .filter(|child| child.kind() == "variable_declarator")
                .filter_map(|declarator| declarator.child_by_field_name("name"))
                .filter(|name| name.kind() == "identifier")
                .map(|name| {
                    make(
                        node,
                        "variable",
                        name.utf8_text(content.as_bytes()).unwrap_or_default(),
                        file,
                    )
                })
                .collect();
            Some(symbols)
        }

        // Python：函数在类体中即为方法；嵌套在函数里的仍是函数。
        ("python", "function_definition") => {
            let kind = if python_in_class(node) {
                "method"
            } else {
                "function"
            };
            single(node, kind, field("name"), file)
        }
        ("python", "class_definition") => single(node, "class", field("name"), file),

        // Rust：impl 块内的 fn 是方法；impl_item 以被实现的类型命名。
        ("rust", "function_item") => {
            let kind = if rust_in_impl(node) {
                "method"
            } else {
                "function"
            };
            single(node, kind, field("name"), file)
        }
        ("rust", "struct_item") => single(node, "class", field("name"), file),
        ("rust", "impl_item") => single(node, "type", field("type"), file),

        // Go：方法有独立的 method_declaration 节点；type_declaration 内含 type_spec。
        ("go", "function_declaration") => single(node, "function", field("name"), file),
        ("go", "method_declaration") => single(node, "method", field("name"), file),
        ("go", "type_spec") => single(node, "type", field("name"), file),

        ("java", "method_declaration") => single(node, "method", field("name"), file),
        ("java", "class_declaration") => single(node, "class", field("name"), file),
        ("java", "interface_declaration") => single(node, "interface", field("name"), file),

        _ => None,
    }
}

/// 构造单符号结果；名字缺失或为空则视为无符号。
fn single(node: &Node, kind: &str, name: Option<String>, file: &str) -> Option<Vec<Symbol>> {
    let name = name?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some(vec![make(node, kind, name, file)])
}

fn make(node: &Node, kind: &str, name: &str, file: &str) -> Symbol {
    Symbol {
        name: name.trim().to_string(),
        kind: kind.to_string(),
        file: file.to_string(),
        start_line: u32::try_from(node.start_position().row).unwrap_or(u32::MAX) + 1, // tree-sitter 行号从 0 计
        end_line: u32::try_from(node.end_position().row).unwrap_or(u32::MAX) + 1,
    }
}

/// 读取节点指定字段的文本。
fn field_text(node: &Node, field: &str, content: &str) -> Option<String> {
    node.child_by_field_name(field).map(|child| {
        child
            .utf8_text(content.as_bytes())
            .unwrap_or_default()
            .to_string()
    })
}

/// Rust：function_item 是否位于 impl 块内（语义上是方法）。
fn rust_in_impl(node: &Node) -> bool {
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        match parent.kind() {
            "impl_item" => return true,
            // 嵌套在外层函数里的 fn 仍是自由函数。
            "function_item" => return false,
            _ => ancestor = parent.parent(),
        }
    }
    false
}

/// Python：function_definition 是否位于类体中（语义上是方法）。
fn python_in_class(node: &Node) -> bool {
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        match parent.kind() {
            "class_definition" => return true,
            "function_definition" | "lambda" => return false,
            _ => ancestor = parent.parent(),
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsx_symbols_keep_crlf_line_numbers_and_ignore_comment_declarations() {
        let source = "// function fake() {}\r\nexport function View() {\r\n  return <div>hello</div>;\r\n}\r\n";
        let symbols = extract_symbols(Path::new("View.tsx"), "typescript", source).unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "View");
        assert_eq!(symbols[0].kind, "function");
        assert_eq!(symbols[0].file, "View.tsx");
        assert_eq!((symbols[0].start_line, symbols[0].end_line), (2, 4));
    }
    use std::path::Path;

    fn find<'a>(symbols: &'a [Symbol], name: &str, kind: &str) -> Option<&'a Symbol> {
        symbols.iter().find(|s| s.name == name && s.kind == kind)
    }

    #[test]
    fn extracts_typescript_symbols_with_exact_lines() {
        // 行 1 为空行，行号以编辑器视角（从 1 开始）计。
        let src = r#"
interface User {
    id: number;
}

type Maybe<T> = T | null;

export function greet(name: string): string {
    return name;
}

class Repo {
    find(id: number) {
        return null;
    }

    helper = () => 1;
}

const a = 1, b = 2;
"#;
        let symbols = extract_symbols(Path::new("src/repo.ts"), "typescript", src).unwrap();

        let user = find(&symbols, "User", "interface").unwrap();
        assert_eq!((user.start_line, user.end_line), (2, 4));
        let maybe = find(&symbols, "Maybe", "type").unwrap();
        assert_eq!((maybe.start_line, maybe.end_line), (6, 6));
        let greet = find(&symbols, "greet", "function").unwrap();
        assert_eq!((greet.start_line, greet.end_line), (8, 10));
        let repo = find(&symbols, "Repo", "class").unwrap();
        assert_eq!((repo.start_line, repo.end_line), (12, 18));
        let find_method = find(&symbols, "find", "method").unwrap();
        assert_eq!((find_method.start_line, find_method.end_line), (13, 15));

        // const a = 1, b = 2 → 两个 variable 符号。
        assert_eq!(find(&symbols, "a", "variable").unwrap().start_line, 20);
        assert_eq!(find(&symbols, "b", "variable").unwrap().start_line, 20);
        // 箭头函数属性不是 method_definition，不应误提。
        assert!(find(&symbols, "helper", "method").is_none());

        assert!(symbols.iter().all(|s| s.file == "src/repo.ts"));
    }

    #[test]
    fn extracts_python_symbols() {
        let src = r#"
class Dog:
    def bark(self):
        pass

    def _roll(self):
        pass

def standalone():
    def nested():
        pass
"#;
        let symbols = extract_symbols(Path::new("dog.py"), "python", src).unwrap();

        assert_eq!(find(&symbols, "Dog", "class").unwrap().start_line, 2);
        let bark = find(&symbols, "bark", "method").unwrap();
        assert_eq!((bark.start_line, bark.end_line), (3, 4));
        assert_eq!(find(&symbols, "_roll", "method").unwrap().start_line, 6);
        let standalone = find(&symbols, "standalone", "function").unwrap();
        assert_eq!((standalone.start_line, standalone.end_line), (9, 11));
        // 嵌套在函数里的 def 仍是 function，不是 method。
        let nested = find(&symbols, "nested", "function").unwrap();
        assert_eq!((nested.start_line, nested.end_line), (10, 11));
    }

    #[test]
    fn extracts_rust_symbols() {
        let src = r#"
struct Config {
    pub name: String,
}

impl Config {
    fn new() -> Self {
        Self { name: String::new() }
    }
}

impl std::fmt::Display for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

fn free_fn() {
    fn inner() {}
}
"#;
        let symbols = extract_symbols(Path::new("src/config.rs"), "rust", src).unwrap();

        let config = find(&symbols, "Config", "class").unwrap(); // struct → class
        assert_eq!((config.start_line, config.end_line), (2, 4));
        assert_eq!(find(&symbols, "new", "method").unwrap().start_line, 7);
        // 两个 impl_item 都以被实现的类型 Config 命名，kind 归为 type。
        let impls: Vec<(u32, u32)> = symbols
            .iter()
            .filter(|s| s.name == "Config" && s.kind == "type")
            .map(|s| (s.start_line, s.end_line))
            .collect();
        assert_eq!(impls, vec![(6, 10), (12, 16)], "两个 impl 块各产出一个符号");
        assert_eq!(find(&symbols, "fmt", "method").unwrap().start_line, 13);
        let free_fn = find(&symbols, "free_fn", "function").unwrap();
        assert_eq!((free_fn.start_line, free_fn.end_line), (18, 20));
        let inner = find(&symbols, "inner", "function").unwrap();
        assert_eq!((inner.start_line, inner.end_line), (19, 19));
    }

    #[test]
    fn extracts_go_symbols() {
        let src = r#"
package main

type Config struct {
    Name string
}

type (
    A int
    B string
)

func Hello() {
}

func (c *Config) String() string {
    return c.Name
}
"#;
        let symbols = extract_symbols(Path::new("main.go"), "go", src).unwrap();

        assert_eq!(find(&symbols, "Config", "type").unwrap().start_line, 4);
        // 分组的 type_declaration：每个 type_spec 一个符号。
        assert_eq!(find(&symbols, "A", "type").unwrap().start_line, 9);
        assert_eq!(find(&symbols, "B", "type").unwrap().start_line, 10);
        assert_eq!(find(&symbols, "Hello", "function").unwrap().start_line, 13);
        let string_method = find(&symbols, "String", "method").unwrap();
        assert_eq!((string_method.start_line, string_method.end_line), (16, 18));
    }

    #[test]
    fn extracts_java_symbols() {
        let src = r#"
public class Service {
    public int compute(int x) {
        return x;
    }
}

interface Repo {
    void save();
}
"#;
        let symbols = extract_symbols(Path::new("Service.java"), "java", src).unwrap();

        let service = find(&symbols, "Service", "class").unwrap();
        assert_eq!((service.start_line, service.end_line), (2, 6));
        let compute = find(&symbols, "compute", "method").unwrap();
        assert_eq!((compute.start_line, compute.end_line), (3, 5));
        let repo = find(&symbols, "Repo", "interface").unwrap();
        assert_eq!((repo.start_line, repo.end_line), (8, 10));
        assert_eq!(find(&symbols, "save", "method").unwrap().start_line, 9);
    }

    #[test]
    fn extracts_javascript_symbols() {
        let src = r#"
class Widget {
    mount() {}
}

function init() {}

const handler = () => {};
let counter = 0;
"#;
        let symbols = extract_symbols(Path::new("app.js"), "javascript", src).unwrap();

        assert_eq!(find(&symbols, "Widget", "class").unwrap().start_line, 2);
        assert_eq!(find(&symbols, "mount", "method").unwrap().start_line, 3);
        assert_eq!(find(&symbols, "init", "function").unwrap().start_line, 6);
        assert_eq!(find(&symbols, "handler", "variable").unwrap().start_line, 8);
        assert_eq!(find(&symbols, "counter", "variable").unwrap().start_line, 9);
    }

    #[test]
    fn unsupported_language_yields_no_symbols() {
        assert!(
            extract_symbols(Path::new("README.md"), "markdown", "# hi")
                .unwrap()
                .is_empty(),
            "markdown 无符号可提"
        );
        assert!(
            extract_symbols(Path::new("data.txt"), "text", "anything")
                .unwrap()
                .is_empty(),
            "text 应返回空 vec"
        );
        assert!(
            extract_symbols(Path::new("x.cbl"), "cobol", "DISPLAY 'X'.")
                .unwrap()
                .is_empty(),
            "未知语言应返回空 vec"
        );
    }

    #[test]
    fn empty_content_yields_no_symbols() {
        for lang in ["typescript", "python", "rust", "go", "java", "javascript"] {
            assert!(extract_symbols(Path::new("empty"), lang, "")
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn syntax_errors_do_not_panic_or_lose_valid_declarations() {
        // 残缺的类体之后，tree-sitter 错误恢复仍能解析出后续声明。
        let src = "function ok() {}\nclass Broken {\n    oops\n}\n\nfunction still() {}\n";
        let symbols = extract_symbols(Path::new("broken.ts"), "typescript", src).unwrap();
        assert!(
            find(&symbols, "ok", "function").is_some(),
            "错误之前的函数应提取"
        );
        assert!(
            find(&symbols, "Broken", "class").is_some(),
            "残缺类本身应提取"
        );
        assert!(
            find(&symbols, "still", "function").is_some(),
            "错误恢复后的函数应提取"
        );
    }
}
