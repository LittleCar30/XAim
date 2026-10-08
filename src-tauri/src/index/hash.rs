//! 内容哈希：基于 blake3 为文本/文件生成稳定的 64 字符小写 hex 摘要，
//! 用于增量索引判重（内容不变 → hash 不变）。

use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use crate::errors::Result;

/// 流式读取的缓冲区大小：64 KiB，大文件分块喂给 hasher，不整块驻留内存。
const READ_BUF_SIZE: usize = 64 * 1024;

/// 计算文本内容的 blake3 摘要，返回 64 字符小写 hex。
pub fn content_hash(content: &str) -> String {
    blake3::hash(content.as_bytes()).to_string()
}

/// 流式计算文件的 blake3 摘要，返回 64 字符小写 hex。
///
/// 读取失败（文件不存在、无权限、是目录等）返回 [`XaimError::Io`]，不 panic。
pub fn file_hash(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; READ_BUF_SIZE];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break; // EOF
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    use crate::errors::XaimError;

    /// BLAKE3 官方测试向量：空输入的摘要。
    const EMPTY_DIGEST: &str = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";

    #[test]
    fn binary_hash_changes_after_same_length_rewrite() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("binary.dat");
        let bytes = [0, 255, 128, 1];
        fs::write(&path, bytes).unwrap();
        let before = file_hash(&path).unwrap();
        assert_eq!(before, blake3::hash(&bytes).to_string());
        fs::write(&path, [0, 255, 128, 2]).unwrap();
        assert_ne!(file_hash(&path).unwrap(), before);
    }

    fn assert_hex64(hash: &str) {
        assert_eq!(hash.len(), 64, "应为 64 字符: {hash}");
        assert!(
            hash.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "应为小写 hex: {hash}"
        );
    }

    #[test]
    fn content_hash_is_stable_and_matches_official_vector() {
        assert_hex64(&content_hash(""));
        assert_eq!(
            content_hash(""),
            EMPTY_DIGEST,
            "空输入应符合 BLAKE3 官方向量"
        );

        let first = content_hash("fn main() {}");
        assert_hex64(&first);
        assert_eq!(content_hash("fn main() {}"), first, "同一内容 hash 应稳定");
    }

    #[test]
    fn different_content_yields_different_hashes() {
        let samples = ["", "a", "ab", "a\n", "你好，世界 🚀"];
        let hashes: Vec<String> = samples.iter().map(|s| content_hash(s)).collect();
        for h in &hashes {
            assert_hex64(h);
        }
        for i in 0..hashes.len() {
            for j in (i + 1)..hashes.len() {
                assert_ne!(
                    hashes[i], hashes[j],
                    "内容 {:?} 与 {:?} 摘要不同",
                    samples[i], samples[j]
                );
            }
        }
    }

    #[test]
    fn file_hash_matches_one_shot_hash_and_streams_large_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("big.txt");

        // 5 MiB，远超 64 KiB 读缓冲，确保走多轮分块。
        let content = "XAim-index-0123456789\n".repeat(5 * 1024 * 1024 / 22);
        fs::write(&path, &content).unwrap();

        let streamed = file_hash(&path).unwrap();
        assert_hex64(&streamed);
        assert_eq!(
            streamed,
            content_hash(&content),
            "流式分块结果应与一次性哈希一致"
        );
    }

    #[test]
    fn file_hash_is_stable_and_handles_empty_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("empty.txt");
        fs::write(&path, b"").unwrap();

        let first = file_hash(&path).unwrap();
        assert_hex64(&first);
        assert_eq!(file_hash(&path).unwrap(), first, "同一文件 hash 应稳定");
        assert_eq!(first, content_hash(""), "空文件应等价于空内容");
    }

    #[test]
    fn file_hash_errors_on_missing_file() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let err = file_hash(&missing).unwrap_err();
        assert!(
            matches!(err, XaimError::Io(_)),
            "失败应返回 Io 错误而非 panic"
        );
    }
}
