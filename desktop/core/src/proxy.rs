//! Proxy for KeyZapper's own internet traffic: the update check and the installer downloads from GitHub, and the Claude
//! installers from claude.ai. In order: `ProxyURL` from IT (`configure`), the proxy environment variables (`HTTPS_PROXY`,
//! `HTTP_PROXY`, `ALL_PROXY`, honouring `NO_PROXY`), then the system proxy (macOS network settings, Windows internet
//! options). An app started from the Finder or the Start menu has no proxy variables, so the system proxy is what a managed
//! Mac or PC usually has. Calls to the gateway and the identity provider stay direct: they are inside the company network.

use std::sync::RwLock;

static MANAGED: RwLock<Option<String>> = RwLock::new(None);

/// The proxy IT sets (`ProxyURL`, e.g. `http://proxy.firma.example:3128`); None or empty = decide from the environment.
pub fn configure(url: Option<String>) {
    if let Ok(mut managed) = MANAGED.write() {
        *managed = url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
    }
}

/// The proxy for internet traffic, None = direct. An unusable `ProxyURL` counts as none, so the request still goes out.
pub fn internet() -> Option<ureq::Proxy> {
    if let Some(url) = MANAGED.read().ok().and_then(|m| m.clone()) {
        return ureq::Proxy::new(&url).ok();
    }
    if let Some(proxy) = ureq::Proxy::try_from_env() {
        return Some(proxy);
    }
    system_proxy().and_then(|url| ureq::Proxy::new(&url).ok())
}

/// `host:port` of the proxy in use, for error messages.
pub fn describe() -> Option<String> {
    internet().map(|p| format!("{}:{}", p.host(), p.port()))
}

/// The HTTPS (else HTTP) proxy of the system settings as a URL.
#[cfg(target_os = "macos")]
fn system_proxy() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/scutil").arg("--proxy").output().ok()?;
    parse_scutil(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(windows)]
fn system_proxy() -> Option<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings").ok()?;
    let enabled: u32 = key.get_value("ProxyEnable").ok()?;
    let server: String = key.get_value("ProxyServer").ok()?;
    parse_windows_proxy(enabled != 0, &server)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn system_proxy() -> Option<String> {
    None
}

/// `scutil --proxy` prints `  HTTPSEnable : 1`, `  HTTPSProxy : host`, `  HTTPSPort : 8080` (and the HTTP triple).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_scutil(text: &str) -> Option<String> {
    let value = |name: &str| {
        text.lines().find_map(|line| {
            let (key, value) = line.split_once(" : ")?;
            (key.trim() == name).then(|| value.trim().to_string())
        })
    };
    for scheme in ["HTTPS", "HTTP"] {
        if value(&format!("{scheme}Enable")).as_deref() != Some("1") {
            continue;
        }
        let host = value(&format!("{scheme}Proxy")).filter(|h| !h.is_empty())?;
        let port = value(&format!("{scheme}Port")).and_then(|p| p.parse::<u16>().ok()).unwrap_or(80);
        return Some(format!("http://{host}:{port}"));
    }
    None
}

/// `ProxyServer` is `host:port` for all protocols, or `http=host:port;https=host:port;…`; `socks=` alone is not used.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_windows_proxy(enabled: bool, server: &str) -> Option<String> {
    if !enabled || server.trim().is_empty() {
        return None;
    }
    if !server.contains('=') {
        return Some(format!("http://{}", server.trim()));
    }
    for scheme in ["https", "http"] {
        if let Some(address) = server.split(';').filter_map(|part| part.trim().split_once('=')).find(|(k, _)| k.trim().eq_ignore_ascii_case(scheme)).map(|(_, v)| v.trim()) {
            if !address.is_empty() {
                return Some(format!("http://{address}"));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_proxy_of_macos_is_read_from_scutil() {
        let text = "<dictionary> {\n  ExceptionsList : <array> {\n    0 : *.local\n  }\n  FTPPassive : 1\n  HTTPEnable : 1\n  HTTPPort : 3128\n  HTTPProxy : proxy.firma.example\n  HTTPSEnable : 1\n  HTTPSPort : 3129\n  HTTPSProxy : sproxy.firma.example\n}\n";
        assert_eq!(parse_scutil(text).as_deref(), Some("http://sproxy.firma.example:3129"));
        let http_only = "<dictionary> {\n  HTTPEnable : 1\n  HTTPPort : 3128\n  HTTPProxy : proxy.firma.example\n  HTTPSEnable : 0\n}\n";
        assert_eq!(parse_scutil(http_only).as_deref(), Some("http://proxy.firma.example:3128"));
        assert_eq!(parse_scutil("<dictionary> {\n  ExceptionsList : <array> {\n    0 : *.local\n  }\n  FTPPassive : 1\n}\n"), None);
    }

    #[test]
    fn the_system_proxy_of_windows_is_read_from_the_internet_options() {
        assert_eq!(parse_windows_proxy(true, "proxy.firma.example:3128").as_deref(), Some("http://proxy.firma.example:3128"));
        assert_eq!(parse_windows_proxy(true, "http=p1:80;https=p2:443;ftp=p3:21").as_deref(), Some("http://p2:443"));
        assert_eq!(parse_windows_proxy(true, "http=p1:80;socks=p4:1080").as_deref(), Some("http://p1:80"));
        assert_eq!(parse_windows_proxy(true, "socks=p4:1080"), None);
        assert_eq!(parse_windows_proxy(false, "proxy.firma.example:3128"), None);
    }

    #[test]
    fn the_proxy_from_it_wins_and_an_unusable_one_means_direct() {
        configure(Some(" http://proxy.firma.example:3128 ".into()));
        let proxy = internet().expect("configured proxy");
        assert_eq!((proxy.host(), proxy.port()), ("proxy.firma.example", 3128));
        assert_eq!(describe().as_deref(), Some("proxy.firma.example:3128"));
        configure(Some("::not a proxy::".into()));
        assert!(internet().is_none());
        configure(None);
    }
}
