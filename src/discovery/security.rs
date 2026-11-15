use crate::common::error::{Error, Result};
use reqwest::Url;
use std::net::{IpAddr, ToSocketAddrs};

/// Security policy for validating discovered provider endpoints.
/// Prevents SSRF attacks against private networks, localhost, link-local,
/// and cloud metadata services.
#[derive(Debug, Clone, Default)]
pub struct EndpointValidator {
    /// Allow private/loopback IPs (strictly for regtest/local test environments).
    pub allow_local_loopback: bool,
}

impl EndpointValidator {
    pub fn new(allow_local_loopback: bool) -> Self {
        Self {
            allow_local_loopback,
        }
    }

    /// Validates an untrusted provider URL string against strict SSRF boundaries.
    pub fn validate_url(&self, raw_url: &str) -> Result<Url> {
        let trimmed = raw_url.trim();
        if trimmed.is_empty() {
            return Err(Error::Config("Endpoint URL cannot be empty".to_string()));
        }

        let parsed = Url::parse(trimmed)
            .map_err(|e| Error::Config(format!("Invalid endpoint URL format: {}", e)))?;

        // 1. Enforce HTTP or HTTPS scheme
        let scheme = parsed.scheme().to_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(Error::Config(format!(
                "Prohibited URL scheme '{}'; only http and https are permitted",
                scheme
            )));
        }

        // 2. Reject credentials in URL (e.g. http://user:pass@host)
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(Error::Config(
                "Endpoint URLs must not contain embedded user credentials".to_string(),
            ));
        }

        // 3. Reject prohibited ports (e.g. standard cloud metadata ports or internal sensitive ports)
        let host_str = parsed
            .host_str()
            .ok_or_else(|| Error::Config("Endpoint URL must contain a valid host".to_string()))?;

        // 4. Validate host IP or domain resolution
        let host_lower = host_str.to_lowercase();
        if host_lower == "localhost" {
            if !self.allow_local_loopback {
                return Err(Error::Config(
                    "Localhost endpoints are rejected by security policy".to_string(),
                ));
            }
            return Ok(parsed);
        }

        // Check if host is direct IP address
        if let Ok(ip) = host_str.parse::<IpAddr>() {
            self.check_ip_address(ip)?;
            return Ok(parsed);
        }

        // If not a direct IP and DNS resolution is possible, verify resolved IPs
        if !self.allow_local_loopback {
            let port = parsed.port_or_known_default().unwrap_or(80);
            let addr_str = format!("{}:{}", host_str, port);
            if let Ok(socket_addrs) = addr_str.to_socket_addrs() {
                for socket_addr in socket_addrs {
                    self.check_ip_address(socket_addr.ip())?;
                }
            }
        }

        Ok(parsed)
    }

    /// Verifies that an IP address is not private, loopback, link-local, multicast, or cloud metadata.
    pub fn check_ip_address(&self, ip: IpAddr) -> Result<()> {
        if self.allow_local_loopback {
            return Ok(());
        }

        match ip {
            IpAddr::V4(ipv4) => {
                if ipv4.is_loopback() {
                    return Err(Error::Config(
                        "Loopback address 127.0.0.0/8 rejected".to_string(),
                    ));
                }
                if ipv4.is_unspecified() {
                    return Err(Error::Config(
                        "Unspecified address 0.0.0.0 rejected".to_string(),
                    ));
                }
                if ipv4.is_link_local() {
                    return Err(Error::Config(
                        "Link-local address 169.254.0.0/16 rejected".to_string(),
                    ));
                }
                if ipv4.is_broadcast() {
                    return Err(Error::Config("Broadcast address rejected".to_string()));
                }
                if ipv4.is_multicast() {
                    return Err(Error::Config("Multicast address rejected".to_string()));
                }
                if ipv4.is_private() {
                    return Err(Error::Config(format!(
                        "Private RFC1918 address '{}' rejected",
                        ipv4
                    )));
                }

                // Cloud metadata IP address: 169.254.169.254
                let octets = ipv4.octets();
                if octets[0] == 169 && octets[1] == 254 && octets[2] == 169 && octets[3] == 254 {
                    return Err(Error::Config(
                        "Cloud instance metadata service address 169.254.169.254 rejected"
                            .to_string(),
                    ));
                }
            }
            IpAddr::V6(ipv6) => {
                if ipv6.is_loopback() {
                    return Err(Error::Config("IPv6 loopback ::1 rejected".to_string()));
                }
                if ipv6.is_unspecified() {
                    return Err(Error::Config(
                        "IPv6 unspecified address :: rejected".to_string(),
                    ));
                }
                if ipv6.is_multicast() {
                    return Err(Error::Config("IPv6 multicast address rejected".to_string()));
                }
                // Unique Local Addresses (fc00::/7, private IPv6 equivalent of RFC1918)
                let segments = ipv6.segments();
                if (segments[0] & 0xfe00) == 0xfc00 {
                    return Err(Error::Config(
                        "IPv6 unique local address (ULA) rejected".to_string(),
                    ));
                }
                // Link-local unicast (fe80::/10)
                if (segments[0] & 0xffc0) == 0xfe80 {
                    return Err(Error::Config(
                        "IPv6 link-local address rejected".to_string(),
                    ));
                }
                // IPv4-mapped IPv6 addresses (e.g. ::ffff:127.0.0.1 or ::ffff:169.254.169.254)
                if let Some(ipv4) = ipv6.to_ipv4_mapped() {
                    return self.check_ip_address(IpAddr::V4(ipv4));
                }
            }
        }

        Ok(())
    }
}
