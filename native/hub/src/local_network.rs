//! 本地网络权限的提示。iOS（以及新版 macOS）访问局域网要用户授权，拒绝后连接
//! 表现为网络错误或超时，看不出原因——失败时判断目标是否在局域网，给出系统
//! 设置的入口。

use std::net::IpAddr;
use std::time::Duration;

use crate::signals::FailureKind;

/// 本地网络开关所在的系统设置页。
#[cfg(target_os = "ios")]
const SETTINGS_URL: &str = "app-settings:";
#[cfg(target_os = "macos")]
const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork";
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
const SETTINGS_URL: &str = "";

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

/// 失败可能源于本地网络权限时返回设置页 URL，否则空串。
pub async fn settings_url(host: &str, port: u16, failure: FailureKind) -> String {
    let relevant = matches!(failure, FailureKind::Network | FailureKind::Timeout);
    if SETTINGS_URL.is_empty() || !relevant || !targets_local_network(host, port).await {
        return String::new();
    }
    SETTINGS_URL.to_owned()
}

async fn targets_local_network(host: &str, port: u16) -> bool {
    let literal = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = literal.parse::<IpAddr>() {
        return is_local(ip);
    }
    if host.trim_end_matches('.').ends_with(".local") {
        return true;
    }
    // 主机名：看它解析到的地址（连接时已解析过一次，这里只在失败后再查）。
    match tokio::time::timeout(LOOKUP_TIMEOUT, tokio::net::lookup_host((host, port))).await {
        Ok(Ok(mut addresses)) => addresses.any(|address| is_local(address.ip())),
        _ => false,
    }
}

/// 局域网地址：私有网段与链路本地地址（回环不需要授权）。
fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_local(IpAddr::V4(v4)),
            None => v6.is_unique_local() || v6.is_unicast_link_local(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{is_local, targets_local_network};

    #[test]
    fn private_and_link_local_addresses_are_local() {
        for address in [
            "10.0.0.8",
            "172.16.4.1",
            "192.168.1.20",
            "169.254.3.3",
            "fd12::1",
            "fe80::1",
            "::ffff:192.168.1.20",
        ] {
            assert!(
                is_local(address.parse().unwrap_or_else(|_| panic!("{address}"))),
                "{address}"
            );
        }
        for address in ["8.8.8.8", "127.0.0.1", "::1", "2001:db8::1", "172.32.0.1"] {
            assert!(
                !is_local(address.parse().unwrap_or_else(|_| panic!("{address}"))),
                "{address}"
            );
        }
    }

    #[tokio::test]
    async fn literals_and_mdns_names_need_no_lookup() {
        assert!(targets_local_network("192.168.1.20", 22).await);
        assert!(targets_local_network("[fe80::1]", 22).await);
        assert!(targets_local_network("nas.local", 22).await);
        assert!(!targets_local_network("127.0.0.1", 22).await);
    }
}
