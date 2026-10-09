//! 构建/测试命令探测：依据项目根目录的特征文件识别工具链与可用验证命令。
//!
//! 多清单项目按 package.json > Cargo.toml > go.mod > pom.xml >
//! requirements.txt/pyproject.toml 的优先级逐字段合并，取第一个检测到的命令。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

/// 项目可用的验证命令集；`None` 表示未检测到对应环节的命令。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VerifyConfig {
    pub type_check: Option<String>,
    pub lint: Option<String>,
    pub test: Option<String>,
    pub build: Option<String>,
}

impl VerifyConfig {
    /// 逐字段合并：已有值的字段保持不变，空字段由 `other` 补齐。
    fn merge(&mut self, other: VerifyConfig) {
        if self.type_check.is_none() {
            self.type_check = other.type_check;
        }
        if self.lint.is_none() {
            self.lint = other.lint;
        }
        if self.test.is_none() {
            self.test = other.test;
        }
        if self.build.is_none() {
            self.build = other.build;
        }
    }
}

/// 探测项目根目录下的验证命令；没有任何已知项目清单时返回全 `None`。
pub fn detect_verify_config(root: &Path) -> VerifyConfig {
    let mut config = VerifyConfig::default();
    for layer in [
        detect_node(root),
        detect_cargo(root),
        detect_go(root),
        detect_maven(root),
        detect_python(root),
    ] {
        config.merge(layer);
    }
    config
}

/// ESLint 配置文件名（覆盖旧版 .eslintrc 与新版 flat config）。
const ESLINT_CONFIG_FILES: &[&str] = &[
    ".eslintrc",
    ".eslintrc.js",
    ".eslintrc.cjs",
    ".eslintrc.json",
    ".eslintrc.yaml",
    ".eslintrc.yml",
    "eslint.config.js",
    "eslint.config.mjs",
    "eslint.config.cjs",
];

/// Node.js 包管理器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageManager {
    Pnpm,
    Yarn,
    Npm,
}

impl PackageManager {
    fn type_check_command(self) -> &'static str {
        match self {
            PackageManager::Pnpm => "pnpm exec tsc --noEmit",
            PackageManager::Yarn => "yarn tsc --noEmit",
            PackageManager::Npm => "node node_modules/typescript/bin/tsc --noEmit",
        }
    }

    fn lint_command(self) -> &'static str {
        match self {
            PackageManager::Pnpm => "pnpm exec eslint .",
            PackageManager::Yarn => "yarn eslint .",
            PackageManager::Npm => "node node_modules/eslint/bin/eslint.js .",
        }
    }

    fn test_command(self) -> &'static str {
        match self {
            PackageManager::Pnpm => "pnpm test",
            PackageManager::Yarn => "yarn test",
            PackageManager::Npm => "npm test",
        }
    }

    fn build_command(self) -> &'static str {
        match self {
            PackageManager::Pnpm => "pnpm build",
            PackageManager::Yarn => "yarn build",
            PackageManager::Npm => "npm run build",
        }
    }

    fn script_command(self, script: &str) -> String {
        match self {
            Self::Pnpm => format!("pnpm run {script}"),
            Self::Yarn => format!("yarn run {script}"),
            Self::Npm => format!("npm run {script}"),
        }
    }
}

/// package.json 中与本探测相关的字段；缺失或解析失败时按无清单处理。
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PackageManifest {
    scripts: BTreeMap<String, serde_json::Value>,
    dependencies: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "devDependencies")]
    dev_dependencies: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "eslintConfig")]
    eslint_config: Option<serde_json::Value>,
}

impl PackageManifest {
    fn has_dependency(&self, name: &str) -> bool {
        self.dependencies.contains_key(name) || self.dev_dependencies.contains_key(name)
    }
}

/// Node.js 项目：依据锁定文件判断包管理器（pnpm > yarn > npm，均缺失时退回 npm）。
fn detect_node(root: &Path) -> VerifyConfig {
    let manifest_path = root.join("package.json");
    if !manifest_path.is_file() {
        return VerifyConfig::default();
    }

    let manifest: Option<PackageManifest> = fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let pm = detect_package_manager(root);
    let has_script = |name: &str| {
        manifest
            .as_ref()
            .is_some_and(|m| m.scripts.contains_key(name))
    };
    let has_dependency = |name: &str| manifest.as_ref().is_some_and(|m| m.has_dependency(name));

    let mut config = VerifyConfig::default();
    if root.join("tsconfig.json").is_file() || has_dependency("typescript") {
        config.type_check = Some(pm.type_check_command().into());
    }
    if let Some(script) = ["typecheck", "type-check"]
        .into_iter()
        .find(|s| has_script(s))
    {
        config.type_check = Some(pm.script_command(script));
    }
    if ESLINT_CONFIG_FILES
        .iter()
        .any(|name| root.join(name).is_file())
        || has_dependency("eslint")
        || manifest.as_ref().is_some_and(|m| m.eslint_config.is_some())
    {
        config.lint = Some(pm.lint_command().into());
    }
    if has_script("lint") {
        config.lint = Some(pm.script_command("lint"));
    }
    if has_script("test") {
        config.test = Some(pm.test_command().into());
    }
    if has_script("build") {
        config.build = Some(pm.build_command().into());
    }
    config
}

/// 按锁定文件/工作区特征判断包管理器，优先级 pnpm > yarn > npm。
fn detect_package_manager(root: &Path) -> PackageManager {
    if root.join("pnpm-lock.yaml").is_file() || root.join("pnpm-workspace.yaml").is_file() {
        PackageManager::Pnpm
    } else if root.join("yarn.lock").is_file() {
        PackageManager::Yarn
    } else {
        PackageManager::Npm
    }
}

/// Rust 项目：cargo 自带 type check（check）、lint（clippy）、test、build。
fn detect_cargo(root: &Path) -> VerifyConfig {
    if !root.join("Cargo.toml").is_file() {
        return VerifyConfig::default();
    }
    let locked = if root.join("Cargo.lock").is_file() {
        " --locked"
    } else {
        ""
    };
    VerifyConfig {
        type_check: Some(format!("cargo check{locked}")),
        lint: Some(format!("cargo clippy{locked}")),
        test: Some(format!("cargo test{locked}")),
        build: Some(format!("cargo build{locked}")),
    }
}

/// Go 项目：go vet 兼作类型检查，Go 无独立 lint 命令。
fn detect_go(root: &Path) -> VerifyConfig {
    if !root.join("go.mod").is_file() {
        return VerifyConfig::default();
    }
    VerifyConfig {
        type_check: Some("go vet ./...".into()),
        lint: None,
        test: Some("go test ./...".into()),
        build: Some("go build ./...".into()),
    }
}

/// Maven 项目：compile 即类型检查，lint 依赖具体插件不在此猜测。
fn detect_maven(root: &Path) -> VerifyConfig {
    if !root.join("pom.xml").is_file() {
        return VerifyConfig::default();
    }
    VerifyConfig {
        type_check: Some("mvn compile".into()),
        lint: None,
        test: Some("mvn test".into()),
        build: Some("mvn package".into()),
    }
}

/// Prefer the framework declared by project configuration or test imports.
fn detect_python(root: &Path) -> VerifyConfig {
    if !root.join("requirements.txt").is_file() && !root.join("pyproject.toml").is_file() {
        return VerifyConfig::default();
    }
    let pytest_config = ["pytest.ini", "conftest.py"]
        .iter()
        .any(|p| root.join(p).is_file())
        || fs::read_to_string(root.join("pyproject.toml"))
            .is_ok_and(|s| s.contains("[tool.pytest."));
    let unittest_tests = fs::read_dir(root.join("tests")).is_ok_and(|entries| {
        entries.take(128).flatten().any(|entry| {
            entry.path().extension().is_some_and(|e| e == "py")
                && fs::metadata(entry.path()).is_ok_and(|m| m.len() <= 128 * 1024)
                && fs::read_to_string(entry.path()).is_ok_and(|s| {
                    s.contains("import unittest") || s.contains("from unittest import")
                })
        })
    });
    VerifyConfig {
        type_check: None,
        lint: None,
        test: Some(
            if unittest_tests && !pytest_config {
                "python -m unittest discover -s tests"
            } else {
                "python -m pytest"
            }
            .into(),
        ),
        build: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::tempdir;

    fn write_file(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// 构造最小 package.json 内容。
    fn package_json(scripts: &[&str], dev_dependencies: &[&str]) -> String {
        let scripts = scripts
            .iter()
            .map(|name| format!("\"{name}\": \"echo {name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let deps = dev_dependencies
            .iter()
            .map(|name| format!("\"{name}\": \"*\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{{\"name\": \"app\", \"scripts\": {{{scripts}}}, \"devDependencies\": {{{deps}}}}}"
        )
    }

    fn node_project(dir: &Path, lock_file: &str) -> VerifyConfig {
        write_file(
            dir,
            "package.json",
            &package_json(&["test", "build"], &["typescript", "eslint"]),
        );
        if !lock_file.is_empty() {
            write_file(dir, lock_file, "");
        }
        write_file(dir, "tsconfig.json", "{}");
        write_file(dir, ".eslintrc.cjs", "");
        detect_verify_config(dir)
    }

    #[test]
    fn empty_root_yields_no_commands() {
        let dir = tempdir().unwrap();
        assert_eq!(detect_verify_config(dir.path()), VerifyConfig::default());
    }

    #[test]
    fn detects_pnpm_node_project() {
        let dir = tempdir().unwrap();
        let config = node_project(dir.path(), "pnpm-lock.yaml");
        assert_eq!(config.type_check.as_deref(), Some("pnpm exec tsc --noEmit"));
        assert_eq!(config.lint.as_deref(), Some("pnpm exec eslint ."));
        assert_eq!(config.test.as_deref(), Some("pnpm test"));
        assert_eq!(config.build.as_deref(), Some("pnpm build"));
    }

    #[test]
    fn detects_yarn_and_npm_package_managers() {
        let yarn_dir = tempdir().unwrap();
        let config = node_project(yarn_dir.path(), "yarn.lock");
        assert_eq!(config.type_check.as_deref(), Some("yarn tsc --noEmit"));
        assert_eq!(config.lint.as_deref(), Some("yarn eslint ."));
        assert_eq!(config.test.as_deref(), Some("yarn test"));
        assert_eq!(config.build.as_deref(), Some("yarn build"));

        let npm_dir = tempdir().unwrap();
        let config = node_project(npm_dir.path(), "package-lock.json");
        assert_eq!(
            config.type_check.as_deref(),
            Some("node node_modules/typescript/bin/tsc --noEmit")
        );
        assert_eq!(
            config.lint.as_deref(),
            Some("node node_modules/eslint/bin/eslint.js .")
        );
        assert_eq!(config.test.as_deref(), Some("npm test"));
        assert_eq!(config.build.as_deref(), Some("npm run build"));
    }

    #[test]
    fn package_manager_priority_prefers_pnpm() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "package.json", &package_json(&["test"], &[]));
        write_file(dir.path(), "pnpm-lock.yaml", "");
        write_file(dir.path(), "yarn.lock", "");
        write_file(dir.path(), "package-lock.json", "{}");

        let config = detect_verify_config(dir.path());
        assert_eq!(config.test.as_deref(), Some("pnpm test"));
    }

    #[test]
    fn node_without_lock_file_falls_back_to_npm_and_scripts_only() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "package.json", &package_json(&["test"], &[]));

        let config = detect_verify_config(dir.path());
        assert_eq!(config.type_check, None);
        assert_eq!(config.lint, None);
        assert_eq!(config.test.as_deref(), Some("npm test"));
        assert_eq!(config.build, None);
    }

    #[test]
    fn node_detects_typescript_and_eslint_from_dependencies() {
        let dir = tempdir().unwrap();
        write_file(
            dir.path(),
            "package.json",
            &package_json(&[], &["typescript", "eslint"]),
        );

        let config = detect_verify_config(dir.path());
        assert_eq!(
            config.type_check.as_deref(),
            Some("node node_modules/typescript/bin/tsc --noEmit")
        );
        assert_eq!(
            config.lint.as_deref(),
            Some("node node_modules/eslint/bin/eslint.js .")
        );
        assert_eq!(config.test, None);
        assert_eq!(config.build, None);
    }

    #[test]
    fn detects_cargo_project() {
        let dir = tempdir().unwrap();
        write_file(
            dir.path(),
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        );

        assert_eq!(
            detect_verify_config(dir.path()).type_check.as_deref(),
            Some("cargo check")
        );
        write_file(dir.path(), "Cargo.lock", "version = 4\n");

        let config = detect_verify_config(dir.path());
        assert_eq!(config.type_check.as_deref(), Some("cargo check --locked"));
        assert_eq!(config.lint.as_deref(), Some("cargo clippy --locked"));
        assert_eq!(config.test.as_deref(), Some("cargo test --locked"));
        assert_eq!(config.build.as_deref(), Some("cargo build --locked"));
    }

    #[test]
    fn detects_go_project() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "go.mod", "module demo\n\ngo 1.21\n");

        let config = detect_verify_config(dir.path());
        assert_eq!(config.type_check.as_deref(), Some("go vet ./..."));
        assert_eq!(config.lint, None);
        assert_eq!(config.test.as_deref(), Some("go test ./..."));
        assert_eq!(config.build.as_deref(), Some("go build ./..."));
    }

    #[test]
    fn detects_maven_project() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "pom.xml", "<project/>");

        let config = detect_verify_config(dir.path());
        assert_eq!(config.type_check.as_deref(), Some("mvn compile"));
        assert_eq!(config.lint, None);
        assert_eq!(config.test.as_deref(), Some("mvn test"));
        assert_eq!(config.build.as_deref(), Some("mvn package"));
    }

    #[test]
    fn detects_python_projects() {
        let req_dir = tempdir().unwrap();
        write_file(req_dir.path(), "requirements.txt", "pytest\n");
        let config = detect_verify_config(req_dir.path());
        assert_eq!(config.type_check, None);
        assert_eq!(config.lint, None);
        assert_eq!(config.test.as_deref(), Some("python -m pytest"));
        assert_eq!(config.build, None);

        let pyproject_dir = tempdir().unwrap();
        write_file(
            pyproject_dir.path(),
            "pyproject.toml",
            "[project]\nname = \"demo\"\n",
        );
        let config = detect_verify_config(pyproject_dir.path());
        assert_eq!(config.test.as_deref(), Some("python -m pytest"));
        assert_eq!(config.type_check, None);
    }

    #[test]
    fn unittest_imports_select_module_entry_and_pytest_config_takes_precedence() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "requirements.txt", "local-library\n");
        write_file(
            dir.path(),
            "tests/test_app.py",
            "import unittest\nclass TestApp(unittest.TestCase): pass\n",
        );
        assert_eq!(
            detect_verify_config(dir.path()).test.as_deref(),
            Some("python -m unittest discover -s tests")
        );
        write_file(dir.path(), "pytest.ini", "[pytest]\n");
        assert_eq!(
            detect_verify_config(dir.path()).test.as_deref(),
            Some("python -m pytest")
        );
    }

    #[test]
    fn mixed_project_merges_fields_by_manifest_priority() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "package.json", &package_json(&["test"], &[]));
        write_file(
            dir.path(),
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        );
        write_file(dir.path(), "Cargo.lock", "version = 4\n");

        let config = detect_verify_config(dir.path());
        assert_eq!(config.type_check.as_deref(), Some("cargo check --locked"));
        assert_eq!(config.lint.as_deref(), Some("cargo clippy --locked"));
        assert_eq!(config.test.as_deref(), Some("npm test"));
        assert_eq!(config.build.as_deref(), Some("cargo build --locked"));
    }
}
