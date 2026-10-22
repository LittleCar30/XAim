//! Process-local Git routing; never writes user or repository Git configuration.
use crate::config::execution::{GitNetwork, ProxyMode};
use tokio::process::Command;

const PROXY_VARS: &[&str] = &[
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
];

pub fn configure(command: &mut Command, network: &GitNetwork) {
    match network.mode {
        ProxyMode::System => {
            // The execution runner clears the global Git config to avoid
            // inherited helpers. Re-import only the user's read-only proxy
            // setting so system mode still works in corporate environments.
            if !PROXY_VARS
                .iter()
                .any(|name| std::env::var_os(name).is_some())
            {
                if let Some(proxy) = system_proxy() {
                    command
                        .env("HTTPS_PROXY", &proxy)
                        .env("https_proxy", &proxy)
                        .env("HTTP_PROXY", &proxy)
                        .env("http_proxy", proxy);
                } else if let Some(proxy) = user_git_proxy() {
                    command.args(["-c", &format!("http.proxy={proxy}")]);
                }
            }
        }
        ProxyMode::Direct => {
            command.args(["-c", "http.proxy="]);
            for name in PROXY_VARS {
                command.env_remove(name);
            }
        }
        ProxyMode::Custom => {
            command.args(["-c", &format!("http.proxy={}", network.url)]);
            command
                .env("HTTPS_PROXY", &network.url)
                .env("https_proxy", &network.url)
                .env("HTTP_PROXY", &network.url)
                .env("http_proxy", &network.url);
        }
    }
}

/// An explicit choice must also override a more specific user URL proxy rule.
pub fn configure_remote(command: &mut Command, network: &GitNetwork, args: &[String]) {
    configure(command, network);
    if network.mode != ProxyMode::System {
        if let Some(url) = args.iter().find(|arg| arg.starts_with("https://")) {
            let proxy = if network.mode == ProxyMode::Direct {
                ""
            } else {
                &network.url
            };
            let url = url.trim_end_matches('/');
            command.args(["-c", &format!("http.{url}/.proxy={proxy}")]);
        }
    }
}

fn user_git_proxy() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["config", "--global", "--get", "http.proxy"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let proxy = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    let url = reqwest::Url::parse(&proxy).ok()?;
    if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.path() != "/" && !url.path().is_empty())
    {
        return None;
    }
    Some(proxy)
}

fn address(value: &str) -> Option<String> {
    let proxy = if value.contains('=') {
        value
            .split(';')
            .filter_map(|entry| entry.split_once('='))
            .find(|(scheme, _)| scheme.trim().eq_ignore_ascii_case("https"))?
            .1
            .trim()
    } else {
        value.trim()
    };
    if proxy.is_empty() {
        return None;
    }
    let url = if proxy.contains("://") {
        proxy.to_owned()
    } else {
        format!("http://{proxy}")
    };
    let settings = GitNetwork {
        mode: ProxyMode::Custom,
        url: url.clone(),
    };
    settings.validate().ok()?;
    Some(url)
}

#[cfg(windows)]
pub fn system_proxy() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };
    let wide = |value: &str| value.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let key = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");
    let enabled_name = wide("ProxyEnable");
    let server_name = wide("ProxyServer");
    let mut enabled: u32 = 0;
    let mut size = 4;
    // RegGetValue validates types and bounds. No handle to close for predefined HKCU.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            enabled_name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut enabled as *mut u32).cast(),
            &mut size,
        )
    };
    if result != 0 || enabled == 0 {
        return None;
    }
    let mut buffer = [0u16; 2048];
    size = (buffer.len() * 2) as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            server_name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if result != 0 {
        return None;
    }
    let end = buffer.iter().position(|v| *v == 0).unwrap_or(buffer.len());
    address(&String::from_utf16_lossy(&buffer[..end]))
}

#[cfg(target_os = "macos")]
pub fn system_proxy() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/scutil")
        .arg("--proxy")
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let field = |name: &str| {
        text.lines()
            .filter_map(|line| line.trim().split_once(" : "))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.trim())
    };
    if field("HTTPSEnable") == Some("1") {
        address(&format!("{}:{}", field("HTTPSProxy")?, field("HTTPSPort")?))
    } else if field("HTTPEnable") == Some("1") {
        address(&format!("{}:{}", field("HTTPProxy")?, field("HTTPPort")?))
    } else {
        None
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn system_proxy() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_single_or_protocol_specific_proxy_without_credentials() {
        assert_eq!(
            address("127.0.0.1:7897"),
            Some("http://127.0.0.1:7897".into())
        );
        assert_eq!(
            address("http=localhost:1;https=localhost:2"),
            Some("http://localhost:2".into())
        );
        assert_eq!(address("http=localhost:1"), None);
        assert_eq!(address("http://name:password@localhost:1"), None);
    }
    #[test]
    fn direct_and_custom_are_process_local() {
        let mut command = Command::new("git");
        configure(
            &mut command,
            &GitNetwork {
                mode: ProxyMode::Custom,
                url: "http://localhost:7897".into(),
            },
        );
        let args: Vec<_> = command.as_std().get_args().collect();
        assert_eq!(args, ["-c", "http.proxy=http://localhost:7897"]);
        let mut command = Command::new("git");
        configure(
            &mut command,
            &GitNetwork {
                mode: ProxyMode::Direct,
                url: String::new(),
            },
        );
        assert!(command
            .as_std()
            .get_envs()
            .any(|(k, v)| k == "HTTPS_PROXY" && v.is_none()));
    }
}
