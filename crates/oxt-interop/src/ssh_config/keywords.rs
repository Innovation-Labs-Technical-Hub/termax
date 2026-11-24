//! ssh_config(5) client keyword table.

/// Canonical spelling of every client keyword this crate understands.
/// Lookup is case-insensitive; the canonical form is used on export.
const KEYWORDS: &[&str] = &[
    "Host",
    "Match",
    "Include",
    "AddKeysToAgent",
    "AddressFamily",
    "BatchMode",
    "BindAddress",
    "BindInterface",
    "CanonicalDomains",
    "CanonicalizeFallbackLocal",
    "CanonicalizeHostname",
    "CanonicalizeMaxDots",
    "CanonicalizePermittedCNAMEs",
    "CASignatureAlgorithms",
    "CertificateFile",
    "ChallengeResponseAuthentication",
    "ChannelTimeout",
    "CheckHostIP",
    "Ciphers",
    "ClearAllForwardings",
    "Compression",
    "ConnectionAttempts",
    "ConnectTimeout",
    "ControlMaster",
    "ControlPath",
    "ControlPersist",
    "DynamicForward",
    "EnableEscapeCommandline",
    "EnableSSHKeysign",
    "EscapeChar",
    "ExitOnForwardFailure",
    "FingerprintHash",
    "ForkAfterAuthentication",
    "ForwardAgent",
    "ForwardX11",
    "ForwardX11Timeout",
    "ForwardX11Trusted",
    "GatewayPorts",
    "GlobalKnownHostsFile",
    "GSSAPIAuthentication",
    "GSSAPIDelegateCredentials",
    "HashKnownHosts",
    "HostbasedAcceptedAlgorithms",
    "HostbasedAuthentication",
    "HostbasedKeyTypes",
    "HostKeyAlgorithms",
    "HostKeyAlias",
    "HostName",
    "IdentitiesOnly",
    "IdentityAgent",
    "IdentityFile",
    "IgnoreUnknown",
    "IPQoS",
    "KbdInteractiveAuthentication",
    "KbdInteractiveDevices",
    "KexAlgorithms",
    "KnownHostsCommand",
    "LocalCommand",
    "LocalForward",
    "LogLevel",
    "LogVerbose",
    "MACs",
    "NoHostAuthenticationForLocalhost",
    "NumberOfPasswordPrompts",
    "ObscureKeystrokeTiming",
    "PasswordAuthentication",
    "PermitLocalCommand",
    "PermitRemoteOpen",
    "PKCS11Provider",
    "Port",
    "PreferredAuthentications",
    "ProxyCommand",
    "ProxyJump",
    "ProxyUseFdpass",
    "PubkeyAcceptedAlgorithms",
    "PubkeyAcceptedKeyTypes",
    "PubkeyAuthentication",
    "RekeyLimit",
    "RemoteCommand",
    "RemoteForward",
    "RequestTTY",
    "RequiredRSASize",
    "RevokedHostKeys",
    "SecurityKeyProvider",
    "SendEnv",
    "ServerAliveCountMax",
    "ServerAliveInterval",
    "SessionType",
    "SetEnv",
    "StdinNull",
    "StreamLocalBindMask",
    "StreamLocalBindUnlink",
    "StrictHostKeyChecking",
    "SyslogFacility",
    "Tag",
    "TCPKeepAlive",
    "Tunnel",
    "TunnelDevice",
    "UpdateHostKeys",
    "User",
    "UserKnownHostsFile",
    "VerifyHostKeyDNS",
    "VersionAddendum",
    "VisualHostKey",
    "XAuthLocation",
];

/// Keywords that may appear several times, each occurrence adding a value.
const MULTI_VALUED: &[&str] = &[
    "identityfile",
    "certificatefile",
    "localforward",
    "remoteforward",
    "dynamicforward",
    "sendenv",
];

/// Keywords whose value is the raw remainder of the line (not split into
/// arguments), matching OpenSSH's handling of command strings.
const RAW_LINE: &[&str] = &[
    "proxycommand",
    "localcommand",
    "remotecommand",
    "knownhostscommand",
];

/// Canonical spelling for a keyword given in any case, if known.
pub(crate) fn canonical(keyword: &str) -> Option<&'static str> {
    KEYWORDS
        .iter()
        .copied()
        .find(|k| k.eq_ignore_ascii_case(keyword))
}

pub(crate) fn is_multi_valued(lower: &str) -> bool {
    MULTI_VALUED.contains(&lower)
}

pub(crate) fn is_raw_line(lower: &str) -> bool {
    RAW_LINE.contains(&lower)
}

/// Keywords that accept `~` and `%` tokens, as `(tilde, tokens)` flags.
/// `HostName` is handled separately because it only accepts `%h` and `%%`.
pub(crate) fn expansion(lower: &str) -> Option<(bool, bool)> {
    match lower {
        "identityfile" | "certificatefile" | "controlpath" | "identityagent"
        | "userknownhostsfile" | "revokedhostkeys" => Some((true, true)),
        "proxycommand" | "localcommand" | "remotecommand" | "knownhostscommand" => {
            Some((false, true))
        }
        _ => None,
    }
}
