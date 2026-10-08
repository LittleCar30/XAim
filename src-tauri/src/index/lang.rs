//! 语言识别：按扩展名把文件映射到语言标识字符串，
//! 供索引层（符号提取）与前端文件树共用同一套语言命名。
//! 另提供项目主语言检测（[`detect_project_language`]），供意图解析提示词标注"项目语言"。

use std::collections::HashMap;
use std::path::Path;

use ignore::WalkBuilder;

use crate::index::scanner::DEFAULT_IGNORES;

/// 扩展名 → 语言 的映射表；扩展名一律小写存储，匹配时不区分大小写。
const EXT_LANGUAGES: &[(&str, &str)] = &[
    ("ts", "typescript"),
    ("tsx", "typescript"),
    ("js", "javascript"),
    ("jsx", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("py", "python"),
    ("rs", "rust"),
    ("go", "go"),
    ("java", "java"),
    ("kt", "kotlin"),
    ("swift", "swift"),
    ("c", "c"),
    ("h", "c"),
    ("cpp", "cpp"),
    ("cc", "cpp"),
    ("hpp", "cpp"),
    ("cs", "csharp"),
    ("rb", "ruby"),
    ("php", "php"),
    ("vue", "vue"),
    ("svelte", "svelte"),
    ("json", "json"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("toml", "toml"),
    ("md", "markdown"),
    ("html", "html"),
    ("css", "css"),
    ("scss", "scss"),
    ("sql", "sql"),
    ("sh", "shell"),
    ("bash", "shell"),
];

/// 无法识别时的语言标识：无扩展名（含 `.gitignore` 等点文件）或未知扩展名。
pub const TEXT: &str = "text";

/// 依据扩展名检测文件语言，大小写不敏感；
/// 无扩展名或未知扩展名返回 [`TEXT`]。
pub fn detect_language(path: &Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    lookup(&ext).to_string()
}

fn lookup(ext: &str) -> &'static str {
    EXT_LANGUAGES
        .iter()
        .find(|(candidate, _)| *candidate == ext)
        .map_or(TEXT, |(_, lang)| lang)
}

/// 项目主语言无法判定时的占位值：目录为空或没有任何可识别文件。
pub const UNKNOWN_PROJECT: &str = "未知";

/// 项目主语言检测最多统计的文件数：只读目录项元数据，取够即止。
const PROJECT_SCAN_CAP: usize = 2000;

/// 检测项目主语言：遍历 `root`（跳过隐藏项、遵守项目内 .gitignore、并过滤
/// [`DEFAULT_IGNORES`] 中的构建产物目录），统计文件扩展名对应的语言，返回
/// 出现最多的一个（并列时取字典序更小者，保证结果确定）。没有任何可识别
/// 文件时返回 [`UNKNOWN_PROJECT`]。只读目录元数据、不读文件内容，最多统计
/// [`PROJECT_SCAN_CAP`] 个文件。
pub fn detect_project_language(root: &Path) -> String {
    let walker = WalkBuilder::new(root)
        .require_git(false) // 与 scanner 一致：非 git 仓库也遵守 .gitignore
        .git_global(false)
        .parents(false)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            !DEFAULT_IGNORES.contains(&name.as_str())
        })
        .build();
    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    let mut scanned = 0usize;
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        if scanned >= PROJECT_SCAN_CAP {
            break;
        }
        scanned += 1;
        let ext = entry
            .path()
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let lang = lookup(&ext);
        if lang != TEXT {
            *counts.entry(lang).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0)))
        .map(|(lang, _)| lang.to_string())
        .unwrap_or_else(|| UNKNOWN_PROJECT.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn project_language_ties_choose_lexically_smallest_language() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("main.rs"), "").unwrap();
        std::fs::write(root.path().join("main.py"), "").unwrap();
        assert_eq!(detect_project_language(root.path()), "python");
        std::fs::write(root.path().join("other.rs"), "").unwrap();
        assert_eq!(detect_project_language(root.path()), "rust");
    }

    #[test]
    fn covers_more_than_twenty_languages() {
        let unique: HashSet<&str> = EXT_LANGUAGES.iter().map(|(_, lang)| *lang).collect();
        assert!(
            unique.len() >= 20,
            "至少支持 20 种语言，当前 {}",
            unique.len()
        );
    }

    #[test]
    fn table_entries_all_resolve() {
        // 表内每条映射都必须能被 detect_language 命中，新增语言时自动被测试覆盖。
        for (ext, lang) in EXT_LANGUAGES {
            let name = format!("file.{ext}");
            let path = Path::new(&name);
            assert_eq!(&detect_language(path), lang, "{name} 应识别为 {lang}");
        }
    }

    #[test]
    fn detects_common_languages() {
        let cases = [
            ("src/main.ts", "typescript"),
            ("App.tsx", "typescript"),
            ("index.js", "javascript"),
            ("plugin.mjs", "javascript"),
            ("legacy.cjs", "javascript"),
            ("script.py", "python"),
            ("lib.rs", "rust"),
            ("main.go", "go"),
            ("App.java", "java"),
            ("Main.kt", "kotlin"),
            ("View.swift", "swift"),
            ("util.c", "c"),
            ("header.h", "c"),
            ("widget.cpp", "cpp"),
            ("widget.cc", "cpp"),
            ("widget.hpp", "cpp"),
            ("Service.cs", "csharp"),
            ("app.rb", "ruby"),
            ("index.php", "php"),
            ("Comp.vue", "vue"),
            ("Page.svelte", "svelte"),
            ("package.json", "json"),
            ("ci.yml", "yaml"),
            ("deploy.yaml", "yaml"),
            ("Cargo.toml", "toml"),
            ("README.md", "markdown"),
            ("index.html", "html"),
            ("style.css", "css"),
            ("theme.scss", "scss"),
            ("query.sql", "sql"),
            ("run.sh", "shell"),
            ("profile.bash", "shell"),
        ];
        for (path, expected) in cases {
            assert_eq!(detect_language(Path::new(path)), expected, "{path}");
        }
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        for path in [
            "MAIN.TS",
            "Main.Ts",
            "Script.PY",
            "LIB.Rs",
            "README.Md",
            "CONFIG.YML",
        ] {
            let detected = detect_language(Path::new(path));
            assert_ne!(detected, TEXT, "{path} 不应因大小写退回 text");
        }
        assert_eq!(detect_language(Path::new("MAIN.TS")), "typescript");
        assert_eq!(detect_language(Path::new("README.MD")), "markdown");
    }

    #[test]
    fn falls_back_to_text() {
        // 无扩展名 / 点文件 / 未知扩展名。
        for path in [
            "Makefile",
            "Dockerfile",
            ".gitignore",
            ".env.local",
            "data.xyz",
            "archive.tar.gz",
            "",
        ] {
            assert_eq!(detect_language(Path::new(path)), TEXT, "{path} 应退回 text");
        }
    }

    #[test]
    fn multi_dot_names_use_last_extension() {
        assert_eq!(detect_language(Path::new("app.min.js")), "javascript");
        assert_eq!(detect_language(Path::new("test.spec.ts")), "typescript");
        assert_eq!(detect_language(Path::new("styles.module.scss")), "scss");
    }

    #[test]
    fn detects_dominant_project_language_ignoring_vendored_dirs() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(root.path().join("src/agent")).unwrap();
        std::fs::write(root.path().join("src/lib.rs"), "").unwrap();
        std::fs::write(root.path().join("src/agent/mod.rs"), "").unwrap();
        std::fs::write(root.path().join("src/main.rs"), "").unwrap();
        std::fs::write(root.path().join("index.ts"), "").unwrap();
        // node_modules 属于 DEFAULT_IGNORES，其中的 js 不应干扰主语言判定。
        std::fs::create_dir_all(root.path().join("node_modules/left-pad")).unwrap();
        for i in 0..10 {
            std::fs::write(
                root.path().join(format!("node_modules/left-pad/f{i}.js")),
                "",
            )
            .unwrap();
        }
        assert_eq!(detect_project_language(root.path()), "rust");
    }

    #[test]
    fn project_language_falls_back_to_unknown() {
        let root = tempfile::tempdir().expect("tempdir");
        // 无扩展名与未知扩展名的文件都不可识别。
        std::fs::write(root.path().join("README"), "").unwrap();
        std::fs::write(root.path().join("data.xyz"), "").unwrap();
        assert_eq!(detect_project_language(root.path()), UNKNOWN_PROJECT);
    }
}
