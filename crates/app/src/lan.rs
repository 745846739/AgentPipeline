//! 局域网访问地址枚举（决策 167）。
//!
//! 手机扫码访问的前提是**给出一个手机真能连上的地址**：回环地址（`127.0.0.1`）
//! 在手机上指向手机自己，扫了必然打不开。本模块枚举本机网卡，挑出可被同网段
//! 设备访问的 IPv4 地址。
//!
//! 两层结构：`rank_lan_addresses` 是纯函数（输入网卡列表，输出排序后的候选地址，
//! 可确定性单测）；`lan_addresses` 只是 `if_addrs::get_if_addrs()` 的薄封装。
//! 择优规则见 `rank_ipv4`——多网卡（VPN / Docker / 虚拟网卡）环境下选错地址的
//! 表现是「二维码扫了打不开」，故这里按可连通性从高到低排序而非碰运气取第一个。

use std::net::{IpAddr, Ipv4Addr};

/// 一个候选局域网地址及其是否值得优先推荐。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanAddress {
    /// 网卡名（如 `en0` / `utun3`），仅用于诊断展示。
    pub interface: String,
    /// 网卡上的 IPv4 地址。
    pub ip: Ipv4Addr,
    /// 是否被判定为「大概率可连」——私网地址且非虚拟网卡。
    pub preferred: bool,
}

/// 枚举本机可用的局域网 IPv4 地址，按推荐度排序。
///
/// 取不到网卡（权限 / 沙箱限制）时返回空表而非报错：分享页据此降级为
/// 「手动填写地址」，不该让一个可选特性拖垮整个页面。
pub fn lan_addresses() -> Vec<LanAddress> {
    let interfaces = match if_addrs::get_if_addrs() {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(error = %e, "枚举本机网卡失败，局域网地址不可用");
            return Vec::new();
        }
    };
    let candidates: Vec<(String, Ipv4Addr, bool)> = interfaces
        .iter()
        .map(|i| {
            (
                i.name.clone(),
                match i.addr.ip() {
                    IpAddr::V4(v4) => Some(v4),
                    IpAddr::V6(_) => None,
                },
                i.is_oper_up(),
            )
        })
        .filter_map(|(name, v4, up)| v4.map(|ip| (name, ip, up)))
        .collect();
    rank_ipv4(&candidates)
}

/// 从网卡快照中挑出并排序可用的局域网地址（决策 167，纯函数便于单测）。
///
/// 入参为 `(网卡名, IPv4 地址, 是否 oper_up)`。筛选与排序规则：
/// ① 丢弃回环与未启用（`oper_up = false`）的网卡——前者手机连不上，后者本就不可达；
/// ② **私网地址优先**（RFC 1918：`10/8`、`172.16/12`、`192.168/16`），公网地址仍
///    列出但排在后面——它通常是对接外网的网卡，同网段设备不一定能访问；
/// ③ 虚拟网卡（VPN / Docker / 虚拟机宿主网卡）降级到最后：名字命中常见虚拟前缀
///    即判定为虚拟，这类地址（如 `utun` 的 `100.64/10` CGNAT 段）在手机上通常打不开。
pub fn rank_ipv4(candidates: &[(String, Ipv4Addr, bool)]) -> Vec<LanAddress> {
    let mut out: Vec<LanAddress> = candidates
        .iter()
        .filter(|(_, ip, up)| *up && !ip.is_loopback() && !ip.is_unspecified())
        .map(|(name, ip, _)| LanAddress {
            interface: name.clone(),
            ip: *ip,
            preferred: is_private(*ip) && !is_virtual_interface(name),
        })
        .collect();

    // 稳定排序：preferred 优先，其次私网地址，最后按网卡名 + 地址保证确定性输出。
    out.sort_by_key(|a| (!a.preferred, !is_private(a.ip), a.interface.clone(), a.ip));
    out
}

/// RFC 1918 私网地址判定。
fn is_private(ip: Ipv4Addr) -> bool {
    ip.is_private()
}

/// 常见虚拟网卡名前缀（VPN / 容器 / 虚拟机宿主网络）。
///
/// 这些网卡上的地址在手机上几乎都连不上，故降级到最后——但仍列出，避免
/// 「什么都没有」比「有个打不开的地址」更难排查。
const VIRTUAL_PREFIXES: &[&str] = &[
    "utun",
    "tun",
    "tap",
    "ipsec",
    "ppp",
    "docker",
    "br-",
    "veth",
    "vmnet",
    "vboxnet",
    "zt",
    "tailscale",
    "wg",
];

fn is_virtual_interface(name: &str) -> bool {
    let lower = name.to_lowercase();
    VIRTUAL_PREFIXES.iter().any(|p| lower.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, ip: &str, up: bool) -> (String, Ipv4Addr, bool) {
        (name.to_string(), ip.parse().unwrap(), up)
    }

    #[test]
    fn loopback_and_down_interfaces_are_dropped() {
        let ranked = rank_ipv4(&[
            c("lo0", "127.0.0.1", true),
            c("en1", "192.168.1.5", false),
            c("en0", "192.168.1.10", true),
        ]);
        assert_eq!(ranked.len(), 1, "回环与未启用网卡都应被丢弃：{ranked:?}");
        assert_eq!(ranked[0].ip, Ipv4Addr::new(192, 168, 1, 10));
    }

    #[test]
    fn private_address_outranks_public() {
        let ranked = rank_ipv4(&[
            c("en0", "203.0.113.7", true), // 公网（文档用保留段）
            c("en1", "10.0.0.12", true),
        ]);
        assert_eq!(ranked[0].ip, Ipv4Addr::new(10, 0, 0, 12), "{ranked:?}");
        assert!(ranked[0].preferred);
        assert!(!ranked[1].preferred, "公网地址不该被标为推荐");
    }

    #[test]
    fn virtual_interfaces_rank_last_despite_private_range() {
        // utun 常见于 macOS VPN，其地址落在 CGNAT 100.64/10，手机连不上
        let ranked = rank_ipv4(&[
            c("utun3", "100.64.0.2", true),
            c("en0", "192.168.1.10", true),
        ]);
        assert_eq!(
            ranked[0].ip,
            Ipv4Addr::new(192, 168, 1, 10),
            "真实网卡应排在虚拟网卡之前：{ranked:?}"
        );
        assert!(ranked[0].preferred);
        assert!(!ranked[1].preferred, "虚拟网卡不该被标为推荐");
    }

    #[test]
    fn ranking_is_deterministic_across_interface_order() {
        let a = rank_ipv4(&[
            c("en1", "192.168.1.11", true),
            c("utun3", "10.8.0.2", true),
            c("en0", "192.168.1.10", true),
        ]);
        let b = rank_ipv4(&[
            c("en0", "192.168.1.10", true),
            c("en1", "192.168.1.11", true),
            c("utun3", "10.8.0.2", true),
        ]);
        assert_eq!(a, b, "网卡枚举顺序不同也应给出同一排序");
    }

    #[test]
    fn all_virtual_still_listed_so_user_can_diagnose() {
        // 只有虚拟网卡时不要返回空表——留一个可排查的地址比什么都没有好
        let ranked = rank_ipv4(&[c("utun3", "10.8.0.2", true)]);
        assert_eq!(ranked.len(), 1);
        assert!(!ranked[0].preferred);
    }

    #[test]
    fn unspecified_address_is_dropped() {
        let ranked = rank_ipv4(&[c("en0", "0.0.0.0", true)]);
        assert!(ranked.is_empty(), "0.0.0.0 不是可访问地址：{ranked:?}");
    }

    #[test]
    fn is_private_covers_rfc1918_ranges() {
        assert!(is_private("10.1.2.3".parse().unwrap()));
        assert!(is_private("172.16.0.1".parse().unwrap()));
        assert!(is_private("172.31.255.254".parse().unwrap()));
        assert!(is_private("192.168.0.1".parse().unwrap()));
        assert!(
            !is_private("172.32.0.1".parse().unwrap()),
            "172.32 不是私网"
        );
        assert!(!is_private("203.0.113.1".parse().unwrap()));
    }
}
