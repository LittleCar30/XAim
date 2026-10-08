//! 统一错误类型：各领域模块与命令层的错误最终都收敛到 [`XaimError`]，
//! 经 `From<XaimError> for String` 转成可读文本返回给前端 IPC。

/// 应用统一错误。
///
/// 基础设施类变体通过 `#[from]` 自动从底层错误类型转换并保留 `source()` 链；
/// 领域类变体（Model/Verify/PatchConflict/Timeout/Config/Security/Unknown）
/// 携带调用方给出的具体 message。
#[derive(thiserror::Error, Debug)]
pub enum XaimError {
    /// 文件系统读写失败。
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    /// SQLite 存储层失败。
    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),
    /// Git 操作失败。
    #[error("Git 错误: {0}")]
    Git(#[from] git2::Error),
    /// 模型 API 的 HTTP 请求失败。
    #[error("网络请求错误: {0}")]
    Http(#[from] reqwest::Error),
    /// JSON 序列化/反序列化失败。
    #[error("JSON 错误: {0}")]
    Json(#[from] serde_json::Error),
    /// 模型提供方返回错误或响应不合法。
    #[error("模型调用错误: {0}")]
    Model(String),
    /// 构建/测试/静态检查执行失败或未通过。
    #[error("验证错误: {0}")]
    Verify(String),
    /// 补丁与当前工作区冲突，无法干净应用。
    #[error("补丁冲突: {0}")]
    PatchConflict(String),
    /// 任务被用户取消，属正常控制流。
    #[error("任务已取消")]
    Cancelled,
    /// 操作超出时限。
    #[error("操作超时: {0}")]
    Timeout(String),
    /// 配置缺失或非法。
    #[error("配置错误: {0}")]
    Config(String),
    /// 操作被安全策略拒绝（如路径越出项目目录）。
    #[error("安全限制: {0}")]
    Security(String),
    /// 未归类的内部错误。
    #[error("未知错误: {0}")]
    Unknown(String),
}

/// 命令层的统一 Result 别名。
pub type Result<T> = std::result::Result<T, XaimError>;

/// IPC 返回约定：错误以字符串形式传给前端，命令层用
/// `Result<T, XaimError>` 配合 `.map_err(String::from)`（或直接 `?` + Into）返回。
impl From<XaimError> for String {
    fn from(err: XaimError) -> Self {
        err.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn payload_variants_have_clear_messages() {
        assert_eq!(XaimError::Cancelled.to_string(), "任务已取消");
        assert_eq!(
            XaimError::Model("429 限流".to_string()).to_string(),
            "模型调用错误: 429 限流"
        );
        assert_eq!(
            XaimError::Verify("test failed".to_string()).to_string(),
            "验证错误: test failed"
        );
        assert_eq!(
            XaimError::PatchConflict("hunk #1".to_string()).to_string(),
            "补丁冲突: hunk #1"
        );
        assert_eq!(
            XaimError::Timeout("30s".to_string()).to_string(),
            "操作超时: 30s"
        );
        assert_eq!(
            XaimError::Config("缺少 provider".to_string()).to_string(),
            "配置错误: 缺少 provider"
        );
        assert_eq!(
            XaimError::Security("路径越出项目目录".to_string()).to_string(),
            "安全限制: 路径越出项目目录"
        );
        assert_eq!(
            XaimError::Unknown("boom".to_string()).to_string(),
            "未知错误: boom"
        );
    }

    #[test]
    fn std_sources_convert_into_variants() {
        let io = XaimError::from(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no such file",
        ));
        assert!(matches!(io, XaimError::Io(_)));
        assert_eq!(io.to_string(), "IO 错误: no such file");

        let db = XaimError::from(rusqlite::Error::InvalidColumnName("name".to_string()));
        assert!(matches!(db, XaimError::Db(_)));

        let git = XaimError::from(git2::Error::from_str("not a repo"));
        assert!(matches!(git, XaimError::Git(_)));

        let json = XaimError::from(serde_json::from_str::<serde_json::Value>("{bad").unwrap_err());
        assert!(matches!(json, XaimError::Json(_)));
    }

    #[tokio::test]
    async fn reqwest_error_converts_into_http_variant() {
        // 非法 URL 让 reqwest 在 send() 处直接返回解析错误，无需真实网络。
        let err = reqwest::get("::::not-a-url").await.unwrap_err();
        assert!(matches!(XaimError::from(err), XaimError::Http(_)));
    }

    #[test]
    fn wrapped_source_is_preserved() {
        let err = XaimError::from(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ));
        let source = err.source().expect("Io 变体应保留底层错误");
        let io_source = source
            .downcast_ref::<std::io::Error>()
            .expect("source 应为 std::io::Error");
        assert_eq!(io_source.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn result_alias_and_string_conversion() {
        let r: Result<u8> = Err(XaimError::Config("缺少 api key".to_string()));
        let text: String = r.unwrap_err().into();
        assert_eq!(text, "配置错误: 缺少 api key");
    }
}
