//! Port of `calibre.utils.certgen` (issue #463): generates a
//! self-signed CA + server certificate/key pair, for `calibre_srv`'s
//! optional self-signed HTTPS.
//!
//! Redesigned around the `rcgen` crate (pure-Rust X.509 generation)
//! rather than porting the 460-line `certgen.c` OpenSSL extension
//! upstream's own `certgen.py` wraps -- per this issue's own filed
//! scope. `rcgen` already implements exactly this shape (a CA cert,
//! then a leaf cert signed by it, with SAN support) as a first-class
//! use case.
//!
//! # Disclosed narrowing / real improvement vs. upstream
//!
//! - Upstream generates RSA key pairs (`create_rsa_keypair`). This
//!   port generates ECDSA P-256 key pairs instead --
//!   `rcgen::KeyPair::generate()`'s own default, and a strictly
//!   better default for a *new* self-signed cert generator (smaller
//!   keys/certs, faster handshakes, no RSA-specific backward-
//!   compatibility need here since nothing in this crate consumes an
//!   existing RSA-keyed certificate). Consistent with this project's
//!   established preference for a real, well-supported modern default
//!   over faithfully reproducing a legacy choice when nothing forces
//!   the legacy choice (see `calibre_utils::icu`'s ICU4X-over-rust_icu
//!   call, issue #459).
//! - `create_cert_request`/`create_cert`/`create_ca_cert` as
//!   *separate* CSR-then-sign steps aren't ported as separate public
//!   functions -- `rcgen`'s own API combines "build params" and
//!   "self-sign" or "sign by an issuer" into one call
//!   (`CertificateParams::self_signed`/`signed_by`), so
//!   [`create_server_cert`] (upstream's own highest-level, actually
//!   *used* function -- `develop()`'s only real caller) is the one
//!   entry point this port provides.
//! - Password-encrypted private key export
//!   (`encrypt_key_with_password`) isn't ported -- `rcgen` serializes
//!   an unencrypted PKCS#8 PEM; no caller in this crate needs
//!   encrypted key storage.
//! - **Not wired into `calibre_srv`'s HTTP listener.** `calibre_srv`
//!   currently serves plain HTTP only (no `rustls`/`native-tls`
//!   dependency, no `--https` option). Adding real HTTPS support is
//!   its own architecture decision (which axum/hyper TLS integration,
//!   whether to auto-generate-and-persist vs. require an operator-
//!   supplied cert, cert renewal) -- out of scope for a "port this
//!   cert-generation primitive" issue. This module is the real,
//!   independently-testable/verifiable building block a future
//!   `calibre_srv --https` feature would use.

use anyhow::Result;
use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, Ia5String, IsCa, KeyPair, KeyUsagePurpose, SanType};
use std::net::IpAddr;
use std::str::FromStr;
use time::{Duration, OffsetDateTime};

/// A generated CA + server certificate/key pair, each field a real
/// PEM-encoded blob ready to write to disk or hand to a TLS library.
pub struct ServerCertBundle {
    pub ca_cert_pem: String,
    pub ca_key_pem: String,
    pub server_cert_pem: String,
    pub server_key_pem: String,
}

pub struct ServerCertOptions {
    pub expire_days: i64,
    pub ca_name: String,
    pub organization: Option<String>,
    /// If empty, derived from `domain_or_ip` (a single `DNS:` or `IP:`
    /// entry, matching upstream's own `if not alt_names: ...` default).
    pub alt_names: Vec<String>,
}

impl Default for ServerCertOptions {
    fn default() -> Self {
        Self { expire_days: 365, ca_name: "Dummy Certificate Authority".to_string(), organization: None, alt_names: Vec::new() }
    }
}

fn san_type_for(name: &str) -> Result<SanType> {
    if let Ok(ip) = IpAddr::from_str(name) {
        Ok(SanType::IpAddress(ip))
    } else {
        Ok(SanType::DnsName(Ia5String::try_from(name.to_string())?))
    }
}

/// Port of `create_server_cert`: builds a fresh self-signed CA, then a
/// server certificate for `domain_or_ip` issued by that CA -- the one
/// entry point upstream's own code actually uses end to end (`develop`,
/// its only real caller, is a manual smoke-test script).
pub fn create_server_cert(domain_or_ip: &str, opts: &ServerCertOptions) -> Result<ServerCertBundle> {
    let now = OffsetDateTime::now_utc();
    let not_after = now + Duration::days(opts.expire_days);

    // Certificate Authority.
    let ca_key = KeyPair::generate()?;
    let mut ca_params = CertificateParams::new(Vec::<String>::new())?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
    ca_params.not_before = now;
    ca_params.not_after = not_after;
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, opts.ca_name.as_str());
    ca_params.distinguished_name = ca_dn;
    let ca_cert = ca_params.self_signed(&ca_key)?;

    // Server certificate, issued by the CA above.
    let alt_names: Vec<String> = if opts.alt_names.is_empty() { vec![domain_or_ip.to_string()] } else { opts.alt_names.clone() };
    let server_key = KeyPair::generate()?;
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    params.key_usages = vec![KeyUsagePurpose::KeyEncipherment, KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
    params.not_before = now;
    params.not_after = not_after;
    params.subject_alt_names = alt_names.iter().map(|n| san_type_for(n)).collect::<Result<Vec<_>>>()?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, domain_or_ip);
    if let Some(org) = &opts.organization {
        dn.push(DnType::OrganizationName, org.as_str());
    }
    params.distinguished_name = dn;
    let server_cert = params.signed_by(&server_key, &ca_cert, &ca_key)?;

    Ok(ServerCertBundle { ca_cert_pem: ca_cert.pem(), ca_key_pem: ca_key.serialize_pem(), server_cert_pem: server_cert.pem(), server_key_pem: server_key.serialize_pem() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use x509_parser::prelude::*;

    /// A minimal PEM->DER decode for these tests -- deliberately not
    /// reusing rcgen's own PEM machinery, so these tests verify the
    /// *actual bytes* `create_server_cert` hands back to a caller
    /// (which only ever sees PEM strings), parsed independently via
    /// `x509-parser` (a real, separate X.509 implementation) rather
    /// than trusting rcgen's own round-trip of its own output.
    fn pem_to_der(pem: &str) -> Vec<u8> {
        let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        base64::engine::general_purpose::STANDARD.decode(b64).unwrap()
    }

    #[test]
    fn generates_a_real_parseable_ca_and_server_certificate() {
        let bundle = create_server_cert("test.example", &ServerCertOptions::default()).unwrap();
        assert!(bundle.ca_cert_pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(bundle.server_cert_pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(bundle.server_key_pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert!(bundle.ca_key_pem.starts_with("-----BEGIN PRIVATE KEY-----"));

        let ca_der = pem_to_der(&bundle.ca_cert_pem);
        let server_der = pem_to_der(&bundle.server_cert_pem);
        let (_, ca_x509) = X509Certificate::from_der(&ca_der).unwrap();
        let (_, server_x509) = X509Certificate::from_der(&server_der).unwrap();
        assert!(ca_x509.is_ca());
        assert!(!server_x509.is_ca());
    }

    #[test]
    fn the_server_certificate_actually_verifies_against_the_generated_ca() {
        let bundle = create_server_cert("verify.example", &ServerCertOptions::default()).unwrap();

        let ca_der = pem_to_der(&bundle.ca_cert_pem);
        let server_der = pem_to_der(&bundle.server_cert_pem);
        let (_, ca_x509) = X509Certificate::from_der(&ca_der).unwrap();
        let (_, server_x509) = X509Certificate::from_der(&server_der).unwrap();

        // Real chain-of-trust checks via an independent X.509
        // implementation: the server cert's issuer really is the CA's
        // subject, and the CA's public key really does validate the
        // server cert's signature (a forged/mismatched signature, or
        // a bug swapping which key signs which cert, would fail this).
        assert_eq!(ca_x509.subject(), server_x509.issuer());
        assert!(server_x509.verify_signature(Some(ca_x509.public_key())).is_ok());
        // The CA's own self-signature also verifies against itself.
        assert!(ca_x509.verify_signature(Some(ca_x509.public_key())).is_ok());
    }

    #[test]
    fn detects_an_ip_address_and_uses_an_ip_san_not_a_dns_san() {
        let bundle = create_server_cert("127.0.0.1", &ServerCertOptions::default()).unwrap();
        let der = pem_to_der(&bundle.server_cert_pem);
        let (_, cert) = X509Certificate::from_der(&der).unwrap();
        let san = cert.subject_alternative_name().unwrap().unwrap().value;
        let names: Vec<_> = san.general_names.iter().collect();
        assert!(matches!(names.as_slice(), [GeneralName::IPAddress(ip)] if *ip == [127, 0, 0, 1]), "expected a single IP SAN, got {names:?}");
    }

    #[test]
    fn detects_a_dns_name_and_uses_a_dns_san_not_an_ip_san() {
        let bundle = create_server_cert("test.example", &ServerCertOptions::default()).unwrap();
        let der = pem_to_der(&bundle.server_cert_pem);
        let (_, cert) = X509Certificate::from_der(&der).unwrap();
        let san = cert.subject_alternative_name().unwrap().unwrap().value;
        let names: Vec<_> = san.general_names.iter().collect();
        assert!(matches!(names.as_slice(), [GeneralName::DNSName(name)] if *name == "test.example"), "expected a single DNS SAN, got {names:?}");
    }

    #[test]
    fn custom_alt_names_are_honored_over_the_default() {
        let opts = ServerCertOptions { alt_names: vec!["alt1.example".to_string(), "alt2.example".to_string()], ..Default::default() };
        let bundle = create_server_cert("main.example", &opts).unwrap();
        let der = pem_to_der(&bundle.server_cert_pem);
        let (_, cert) = X509Certificate::from_der(&der).unwrap();
        let san = cert.subject_alternative_name().unwrap().unwrap().value;
        let names: Vec<String> = san
            .general_names
            .iter()
            .filter_map(|n| match n {
                GeneralName::DNSName(s) => Some(s.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["alt1.example", "alt2.example"]);
    }

    #[test]
    fn the_ca_common_name_and_expiry_are_applied() {
        let opts = ServerCertOptions { ca_name: "My Test CA".to_string(), expire_days: 30, ..Default::default() };
        let bundle = create_server_cert("test.example", &opts).unwrap();
        let ca_der = pem_to_der(&bundle.ca_cert_pem);
        let (_, ca_x509) = X509Certificate::from_der(&ca_der).unwrap();
        assert_eq!(ca_x509.subject().to_string(), "CN=My Test CA");

        let validity_days = (ca_x509.validity().not_after.timestamp() - ca_x509.validity().not_before.timestamp()) / 86400;
        assert_eq!(validity_days, 30);
    }
}
