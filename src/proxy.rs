//! Proxy lookup shared by the Nominal LabVIEW libraries.
//!
//! This file is identical in nominal-streaming-labview-ffi and reqwest-labview
//! so both libraries pick a proxy the same way. Change both copies together.
//!
//! The proxy for a destination comes from, in order:
//!
//! 1. The proxy passed by the caller (`direct` means no proxy), used for every
//!    destination.
//! 2. `HTTPS_PROXY` for https:// destinations or `HTTP_PROXY` for http://
//!    ones, then `ALL_PROXY`. Hosts in `NO_PROXY` skip it.
//! 3. The Windows proxy setting (HKCU Internet Settings: `ProxyEnable`,
//!    `ProxyServer`). Hosts in `ProxyOverride`, or in `NO_PROXY` when that is
//!    set, skip it.
//!
//! Only a fixed `http://host:port` proxy with optional Basic credentials is
//! supported. PAC scripts, automatic detection (WPAD), SOCKS, and NTLM or
//! Kerberos proxy authentication are not. A proxy setting that cannot be used
//! is an error rather than being skipped.

use percent_encoding::percent_decode_str;
use url::Url;

/// A proxy server to connect through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyServer {
    pub host: String,
    pub port: u16,
    /// Basic authentication username and password.
    pub credentials: Option<(String, String)>,
}

/// A proxy and the hosts that skip it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Route {
    server: ProxyServer,
    bypass: Option<String>,
}

/// Whether a proxy setting asks for no proxy (`direct`, in any case).
pub fn is_direct(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("direct")
}

/// The proxies to use, looked up once and then chosen per destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxySettings {
    http: Option<Route>,
    https: Option<Route>,
}

impl ProxySettings {
    /// Looks up the proxies. `explicit` is the caller's value, if any.
    pub fn detect(explicit: Option<&str>) -> Result<ProxySettings, String> {
        detect_from(explicit, env_var, system_proxy)
    }

    /// The proxy for a destination, or `None` to connect directly.
    pub fn for_destination(&self, scheme: &str, host: &str) -> Option<&ProxyServer> {
        let route = match scheme {
            "http" => self.http.as_ref(),
            "https" => self.https.as_ref(),
            _ => None,
        }?;
        if route
            .bypass
            .as_deref()
            .is_some_and(|list| bypasses(list, host))
        {
            return None;
        }
        Some(&route.server)
    }
}

/// The Windows proxy setting, read only when it is turned on.
#[cfg_attr(not(windows), allow(dead_code))]
struct SystemProxy {
    server: String,
    bypass: Option<String>,
}

fn detect_from(
    explicit: Option<&str>,
    env: impl Fn(&str) -> Option<String>,
    system: impl FnOnce() -> Option<SystemProxy>,
) -> Result<ProxySettings, String> {
    if let Some(value) = explicit.map(str::trim).filter(|v| !v.is_empty()) {
        if is_direct(value) {
            return Ok(ProxySettings {
                http: None,
                https: None,
            });
        }
        let server = parse_proxy(value).map_err(|e| format!("Invalid proxy: {}", e))?;
        let route = Route {
            server,
            bypass: None,
        };
        return Ok(ProxySettings {
            http: Some(route.clone()),
            https: Some(route),
        });
    }

    let no_proxy = first_env(&env, &["NO_PROXY", "no_proxy"]).map(|(_, v)| v);
    let system = system();
    let route =
        |scheme, names| detect_route(scheme, names, &env, no_proxy.as_deref(), system.as_ref());
    Ok(ProxySettings {
        http: route("http", ["HTTP_PROXY", "http_proxy"])?,
        https: route("https", ["HTTPS_PROXY", "https_proxy"])?,
    })
}

fn detect_route(
    scheme: &str,
    scheme_names: [&'static str; 2],
    env: &impl Fn(&str) -> Option<String>,
    no_proxy: Option<&str>,
    system: Option<&SystemProxy>,
) -> Result<Option<Route>, String> {
    let names = [scheme_names[0], scheme_names[1], "ALL_PROXY", "all_proxy"];
    if let Some((name, value)) = first_env(env, &names) {
        let server =
            parse_proxy(&value).map_err(|e| format!("Invalid proxy in {}: {}", name, e))?;
        return Ok(Some(Route {
            server,
            bypass: no_proxy.map(str::to_string),
        }));
    }

    let Some(system) = system else {
        return Ok(None);
    };
    let Some(address) = entry_for(&system.server, scheme) else {
        return Ok(None);
    };
    let server = parse_proxy(address)
        .map_err(|e| format!("Invalid proxy in the Windows proxy setting: {}", e))?;
    // Like reqwest, NO_PROXY takes the place of the Windows bypass list when set.
    let bypass = no_proxy
        .map(str::to_string)
        .or_else(|| system.bypass.clone());
    Ok(Some(Route { server, bypass }))
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn first_env(
    env: &impl Fn(&str) -> Option<String>,
    names: &[&'static str],
) -> Option<(&'static str, String)> {
    names.iter().find_map(|&name| {
        env(name)
            .filter(|v| !v.trim().is_empty())
            .map(|v| (name, v.trim().to_string()))
    })
}

#[cfg(windows)]
fn system_proxy() -> Option<SystemProxy> {
    let settings = windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    if settings.get_u32("ProxyEnable").unwrap_or(0) == 0 {
        return None;
    }
    Some(SystemProxy {
        server: settings.get_string("ProxyServer").ok()?,
        bypass: settings.get_string("ProxyOverride").ok(),
    })
}

#[cfg(not(windows))]
fn system_proxy() -> Option<SystemProxy> {
    None
}

/// Parses `host:port` or `http://[user:password@]host[:port]`.
///
/// Errors never include the value, which may hold a password.
fn parse_proxy(value: &str) -> Result<ProxyServer, String> {
    let with_scheme = if value.contains("://") {
        value.to_string()
    } else {
        format!("http://{}", value)
    };
    let url = Url::parse(&with_scheme).map_err(|e| e.to_string())?;
    if url.scheme() != "http" {
        return Err(format!(
            "{}:// proxies are not supported, only http://",
            url.scheme()
        ));
    }
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or("missing host")?;
    let port = url.port_or_known_default().ok_or("missing port")?;

    let credentials = if url.username().is_empty() {
        None
    } else {
        let username = decode(url.username())?;
        let password = decode(url.password().unwrap_or(""))?;
        Some((username, password))
    };

    Ok(ProxyServer {
        host: host.to_string(),
        port,
        credentials,
    })
}

fn decode(value: &str) -> Result<String, String> {
    percent_decode_str(value)
        .decode_utf8()
        .map(|s| s.into_owned())
        .map_err(|_| "credentials are not valid UTF-8".to_string())
}

/// Picks the entry for `scheme` from a Windows `ProxyServer` value.
///
/// The value is either one `host:port` used for every protocol, or a list such
/// as `http=a:80;https=b:443`. In a list, a scheme uses its own entry or else
/// an entry with no protocol; with neither, it connects directly.
fn entry_for<'a>(server: &'a str, scheme: &str) -> Option<&'a str> {
    let mut any_protocol = None;
    for entry in server
        .split(|c: char| c == ';' || c.is_whitespace())
        .filter(|e| !e.is_empty())
    {
        match entry.split_once('=') {
            Some((protocol, address)) if protocol.eq_ignore_ascii_case(scheme) => {
                return Some(address).filter(|a| !a.is_empty());
            }
            Some(_) => {}
            None => {
                any_protocol.get_or_insert(entry);
            }
        }
    }
    any_protocol
}

/// Whether `host` is in a bypass list.
///
/// Accepts both `NO_PROXY` (`a.com,.b.com`) and Windows `ProxyOverride`
/// (`*.a.com;<local>`) syntax. IP ranges are not supported.
fn bypasses(list: &str, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    list.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|e| !e.is_empty())
        .any(|entry| {
            let entry = entry.to_ascii_lowercase();
            if entry == "<local>" {
                !host.contains('.')
            } else if entry.contains('*') {
                wildcard_match(&entry, &host)
            } else {
                let domain = entry.trim_start_matches('.');
                host == domain || host.ends_with(&format!(".{}", domain))
            }
        })
}

/// Matches `text` against a pattern where `*` matches any run of characters.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const HOST: &str = "api.gov.nominal.io";

    fn server(host: &str, port: u16) -> ProxyServer {
        ProxyServer {
            host: host.to_string(),
            port,
            credentials: None,
        }
    }

    fn detect_with(
        explicit: Option<&str>,
        env: &[(&str, &str)],
        system: Option<(&str, Option<&str>)>,
    ) -> Result<ProxySettings, String> {
        let env: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        detect_from(
            explicit,
            |name| env.get(name).cloned(),
            || {
                system.map(|(server, bypass)| SystemProxy {
                    server: server.to_string(),
                    bypass: bypass.map(str::to_string),
                })
            },
        )
    }

    /// The proxy chosen for `scheme://HOST`.
    fn proxy_for(
        scheme: &str,
        explicit: Option<&str>,
        env: &[(&str, &str)],
        system: Option<(&str, Option<&str>)>,
    ) -> Result<Option<ProxyServer>, String> {
        detect_with(explicit, env, system).map(|s| s.for_destination(scheme, HOST).cloned())
    }

    #[test]
    fn parse_proxy_accepts_supported_forms() {
        assert_eq!(
            parse_proxy("proxy.corp:8080"),
            Ok(server("proxy.corp", 8080))
        );
        assert_eq!(
            parse_proxy("http://proxy.corp:3128"),
            Ok(server("proxy.corp", 3128))
        );
        assert_eq!(
            parse_proxy("http://proxy.corp"),
            Ok(server("proxy.corp", 80))
        );
        assert_eq!(
            parse_proxy("http://proxy.corp:3128/"),
            Ok(server("proxy.corp", 3128))
        );
        assert_eq!(
            parse_proxy("http://us%40er:p%3Ass@proxy.corp:8080"),
            Ok(ProxyServer {
                credentials: Some(("us@er".to_string(), "p:ss".to_string())),
                ..server("proxy.corp", 8080)
            })
        );
    }

    #[test]
    fn parse_proxy_rejects_unsupported_forms() {
        assert!(parse_proxy("https://proxy.corp:443")
            .unwrap_err()
            .contains("https://"));
        assert!(parse_proxy("socks5://proxy.corp:1080")
            .unwrap_err()
            .contains("socks5://"));
        assert!(parse_proxy("http://").is_err());
    }

    #[test]
    fn parse_proxy_errors_leave_out_the_password() {
        let err = parse_proxy("https://user:hunter2@proxy.corp:443").unwrap_err();
        assert!(!err.contains("hunter2"), "{}", err);
    }

    #[test]
    fn entry_for_handles_windows_proxy_server_forms() {
        assert_eq!(
            entry_for("proxy.corp:8080", "https"),
            Some("proxy.corp:8080")
        );
        assert_eq!(
            entry_for("proxy.corp:8080", "http"),
            Some("proxy.corp:8080")
        );
        assert_eq!(entry_for("http=a:80;https=b:443", "https"), Some("b:443"));
        assert_eq!(entry_for("http=a:80;https=b:443", "http"), Some("a:80"));
        assert_eq!(entry_for("HTTPS=b:443", "https"), Some("b:443"));
        assert_eq!(
            entry_for("https=http://b:443", "https"),
            Some("http://b:443")
        );
        assert_eq!(entry_for("http=a:80;b:8080", "https"), Some("b:8080"));
        assert_eq!(entry_for("http=a:80;ftp=c:21", "https"), None);
        assert_eq!(entry_for("socks=s:1080", "https"), None);
        assert_eq!(entry_for("", "https"), None);
    }

    #[test]
    fn bypasses_matches_no_proxy_and_proxy_override_entries() {
        assert!(bypasses("*", HOST));
        assert!(bypasses("nominal.io", HOST));
        assert!(bypasses(".nominal.io", HOST));
        assert!(bypasses("other.com, api.gov.nominal.io", HOST));
        assert!(bypasses("localhost;*.nominal.io", HOST));
        assert!(bypasses("api.*.nominal.io", HOST));
        assert!(bypasses("API.GOV.NOMINAL.IO", HOST));
        assert!(bypasses("<local>", "intranet"));

        assert!(!bypasses("<local>", HOST));
        assert!(!bypasses("minal.io", HOST));
        assert!(!bypasses("*.example.com;10.*", HOST));
        assert!(!bypasses("", HOST));
    }

    #[test]
    fn explicit_proxy_wins() {
        let env = [("HTTPS_PROXY", "env.corp:1")];
        let system = Some(("system.corp:2", None));
        assert_eq!(
            proxy_for("https", Some("explicit.corp:3"), &env, system),
            Ok(Some(server("explicit.corp", 3)))
        );
        assert_eq!(
            proxy_for("http", Some("explicit.corp:3"), &env, system),
            Ok(Some(server("explicit.corp", 3)))
        );
        assert_eq!(proxy_for("https", Some("DIRECT"), &env, system), Ok(None));
        assert_eq!(
            proxy_for("https", Some("  "), &env, system),
            Ok(Some(server("env.corp", 1)))
        );
    }

    #[test]
    fn explicit_proxy_ignores_bypass_lists() {
        let env = [("NO_PROXY", "*")];
        let system = Some(("system.corp:2", Some("*")));
        assert_eq!(
            proxy_for("https", Some("explicit.corp:3"), &env, system),
            Ok(Some(server("explicit.corp", 3)))
        );
    }

    #[test]
    fn environment_wins_over_windows_setting() {
        let system = Some(("system.corp:2", None));
        assert_eq!(
            proxy_for("https", None, &[("ALL_PROXY", "all.corp:4")], system),
            Ok(Some(server("all.corp", 4)))
        );
        assert_eq!(
            proxy_for(
                "https",
                None,
                &[("ALL_PROXY", "all.corp:4"), ("HTTPS_PROXY", "https.corp:5")],
                system
            ),
            Ok(Some(server("https.corp", 5)))
        );
        assert_eq!(
            proxy_for("https", None, &[("https_proxy", "lower.corp:6")], None),
            Ok(Some(server("lower.corp", 6)))
        );
    }

    #[test]
    fn each_scheme_uses_its_own_variable() {
        let env = [
            ("HTTP_PROXY", "http.corp:7"),
            ("HTTPS_PROXY", "https.corp:8"),
        ];
        assert_eq!(
            proxy_for("http", None, &env, None),
            Ok(Some(server("http.corp", 7)))
        );
        assert_eq!(
            proxy_for("https", None, &env, None),
            Ok(Some(server("https.corp", 8)))
        );
        // Without its own variable, a scheme falls back to the Windows setting.
        assert_eq!(
            proxy_for(
                "https",
                None,
                &[("HTTP_PROXY", "http.corp:7")],
                Some(("system.corp:2", None))
            ),
            Ok(Some(server("system.corp", 2)))
        );
        assert_eq!(proxy_for("ftp", None, &env, None), Ok(None));
    }

    #[test]
    fn no_proxy_skips_environment_proxy() {
        let env = [("HTTPS_PROXY", "env.corp:1"), ("NO_PROXY", ".nominal.io")];
        assert_eq!(proxy_for("https", None, &env, None), Ok(None));
        let settings = detect_with(None, &env, None).unwrap();
        assert_eq!(
            settings.for_destination("https", "example.com"),
            Some(&server("env.corp", 1))
        );
    }

    #[test]
    fn windows_setting_is_used_without_environment() {
        assert_eq!(
            proxy_for("https", None, &[], Some(("system.corp:2", Some("<local>")))),
            Ok(Some(server("system.corp", 2)))
        );
        assert_eq!(
            proxy_for("https", None, &[], Some(("http=a:80;https=b:443", None))),
            Ok(Some(server("b", 443)))
        );
        assert_eq!(
            proxy_for("http", None, &[], Some(("http=a:80;https=b:443", None))),
            Ok(Some(server("a", 80)))
        );
    }

    #[test]
    fn windows_bypass_list_skips_windows_proxy() {
        assert_eq!(
            proxy_for(
                "https",
                None,
                &[],
                Some(("system.corp:2", Some("*.nominal.io;<local>")))
            ),
            Ok(None)
        );
        // NO_PROXY replaces the Windows list rather than adding to it.
        assert_eq!(
            proxy_for(
                "https",
                None,
                &[("NO_PROXY", "other.com")],
                Some(("system.corp:2", Some("*.nominal.io")))
            ),
            Ok(Some(server("system.corp", 2)))
        );
    }

    #[test]
    fn windows_setting_without_scheme_entry_connects_directly() {
        assert_eq!(
            proxy_for("https", None, &[], Some(("http=a:80", None))),
            Ok(None)
        );
    }

    #[test]
    fn no_proxy_configured_connects_directly() {
        assert_eq!(proxy_for("https", None, &[], None), Ok(None));
        assert_eq!(proxy_for("http", None, &[], None), Ok(None));
    }

    #[test]
    fn invalid_detected_proxy_names_its_source() {
        let err = detect_with(None, &[("HTTPS_PROXY", "socks5://s:1080")], None).unwrap_err();
        assert!(err.contains("HTTPS_PROXY"), "{}", err);
        let err = detect_with(None, &[("HTTP_PROXY", "socks5://s:1080")], None).unwrap_err();
        assert!(err.contains("HTTP_PROXY"), "{}", err);
        let err = detect_with(None, &[], Some(("https=socks5://s:1080", None))).unwrap_err();
        assert!(err.contains("Windows proxy setting"), "{}", err);
    }
}
