//! Stable `TMX-<AREA>-NNN` error codes with Vietnamese and English text,
//! suggested actions and retry classes, and the mapping from transport
//! failures to codes.
//!
//! The catalogue is data: [`CATALOGUE`] has one row per code, and every code
//! has a section in `docs/error-codes.md` (a test checks both). Codes are
//! never renumbered or reused; retire a code by leaving its row in place.

use std::fmt;
use std::io;

use termax_ssh::{CloseReason, Error as SshError};

/// A stable error code. [`ErrorCode::as_str`] is the user-visible form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ErrorCode {
    /// `TMX-NET-001` connection timed out.
    NetTimeout,
    /// `TMX-NET-002` connection refused.
    NetRefused,
    /// `TMX-NET-003` host or network unreachable.
    NetUnreachable,
    /// `TMX-NET-004` connection lost.
    NetLost,
    /// `TMX-NET-005` server stopped answering keepalives.
    NetKeepalive,
    /// `TMX-NET-006` server closed the connection.
    NetServerClosed,
    /// `TMX-DNS-001` name resolution timed out.
    DnsTimeout,
    /// `TMX-DNS-002` name could not be resolved.
    DnsNotFound,
    /// `TMX-HOSTKEY-001` host key changed.
    HostKeyChanged,
    /// `TMX-HOSTKEY-002` host key revoked.
    HostKeyRevoked,
    /// `TMX-HOSTKEY-003` unknown host key rejected.
    HostKeyRejected,
    /// `TMX-HOSTKEY-004` known-hosts store unavailable.
    HostKeyStore,
    /// `TMX-AUTH-001` authentication failed.
    AuthFailed,
    /// `TMX-AUTH-002` server skipped authentication (trivial auth refused).
    AuthTrivial,
    /// `TMX-AUTH-003` private key unusable.
    AuthKeyLoad,
    /// `TMX-AUTH-004` authentication cancelled by the user.
    AuthCancelled,
    /// `TMX-SSH-001` SSH protocol error.
    SshProtocol,
    /// `TMX-SSH-002` server refused a channel.
    SshChannelRefused,
    /// `TMX-SSH-003` operation not supported.
    SshUnsupported,
    /// `TMX-SSH-004` unexpected SSH error.
    SshUnexpected,
    /// `TMX-CFG-001` invalid connection settings.
    ConfigInvalid,
    /// `TMX-FWD-001` port forward could not listen.
    ForwardBind,
}

impl ErrorCode {
    /// The code as shown to users, e.g. `TMX-NET-002`.
    pub fn as_str(self) -> &'static str {
        self.entry().id
    }

    /// The catalogue row for this code.
    pub fn entry(self) -> &'static CatalogueEntry {
        CATALOGUE
            .iter()
            .find(|e| e.code == self)
            .unwrap_or_else(|| unreachable!("every ErrorCode has a catalogue row"))
    }

    /// Relative link to the code's VI/EN documentation section.
    pub fn doc_link(self) -> String {
        format!("{DOC_PAGE}#{}", self.as_str().to_ascii_lowercase())
    }

    /// Whether a session failing with this code is reconnected automatically.
    pub fn retry(self) -> Retry {
        self.entry().retry
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The documentation page, relative to the repository root.
pub const DOC_PAGE: &str = "docs/error-codes.md";

/// Whether automatic reconnect may follow a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// Transient (network, keepalive, server close): reconnect with backoff.
    Transient,
    /// Never retried automatically: host-key, authentication and
    /// configuration problems need the user.
    Never,
}

/// Display language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// English.
    En,
    /// Vietnamese.
    Vi,
}

/// A suggested next step shown with an error (never a modal dialog).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SuggestedAction {
    /// Try again now.
    Retry,
    /// Check the network connection, VPN or proxy.
    CheckNetwork,
    /// Check the host name, address and port.
    CheckAddress,
    /// Open the host-key trust sheet.
    ReviewHostKey,
    /// Check the user name, keys or agent.
    CheckCredentials,
    /// Edit the host or connection settings.
    EditSettings,
    /// Ask the server administrator.
    ContactAdmin,
}

impl SuggestedAction {
    /// Button or link label.
    pub fn label(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Retry, Lang::En) => "Try again",
            (Self::Retry, Lang::Vi) => "Thử lại",
            (Self::CheckNetwork, Lang::En) => "Check the network, VPN or proxy",
            (Self::CheckNetwork, Lang::Vi) => "Kiểm tra mạng, VPN hoặc proxy",
            (Self::CheckAddress, Lang::En) => "Check the host address and port",
            (Self::CheckAddress, Lang::Vi) => "Kiểm tra địa chỉ và cổng của máy chủ",
            (Self::ReviewHostKey, Lang::En) => "Review the host key",
            (Self::ReviewHostKey, Lang::Vi) => "Xem lại khóa máy chủ",
            (Self::CheckCredentials, Lang::En) => "Check the user name and keys",
            (Self::CheckCredentials, Lang::Vi) => "Kiểm tra tên người dùng và khóa",
            (Self::EditSettings, Lang::En) => "Edit the connection settings",
            (Self::EditSettings, Lang::Vi) => "Sửa cài đặt kết nối",
            (Self::ContactAdmin, Lang::En) => "Contact the server administrator",
            (Self::ContactAdmin, Lang::Vi) => "Liên hệ quản trị viên máy chủ",
        }
    }
}

/// Title and one-sentence explanation in one language.
#[derive(Debug, Clone, Copy)]
pub struct Text {
    /// Short title.
    pub title: &'static str,
    /// What happened and why, in one or two sentences.
    pub message: &'static str,
}

/// One catalogue row.
#[derive(Debug)]
pub struct CatalogueEntry {
    /// The code.
    pub code: ErrorCode,
    /// User-visible id, `TMX-<AREA>-NNN`.
    pub id: &'static str,
    /// Retry class.
    pub retry: Retry,
    /// English text.
    pub en: Text,
    /// Vietnamese text.
    pub vi: Text,
    /// Suggested actions, most useful first.
    pub actions: &'static [SuggestedAction],
}

impl CatalogueEntry {
    /// Text in `lang`.
    pub fn text(&self, lang: Lang) -> Text {
        match lang {
            Lang::En => self.en,
            Lang::Vi => self.vi,
        }
    }
}

use SuggestedAction as A;

macro_rules! row {
    ($code:ident, $id:literal, $retry:ident, ($en_t:literal, $en_m:literal), ($vi_t:literal, $vi_m:literal), [$($a:ident),*]) => {
        CatalogueEntry {
            code: ErrorCode::$code,
            id: $id,
            retry: Retry::$retry,
            en: Text { title: $en_t, message: $en_m },
            vi: Text { title: $vi_t, message: $vi_m },
            actions: &[$(A::$a),*],
        }
    };
}

/// Every error code, in display order.
pub static CATALOGUE: &[CatalogueEntry] = &[
    row!(
        NetTimeout,
        "TMX-NET-001",
        Transient,
        ("Connection timed out", "The server did not answer in time."),
        ("Hết thời gian kết nối", "Máy chủ không phản hồi kịp thời."),
        [Retry, CheckNetwork, CheckAddress]
    ),
    row!(
        NetRefused,
        "TMX-NET-002",
        Transient,
        (
            "Connection refused",
            "The server refused the connection; no SSH service is listening on that port."
        ),
        (
            "Kết nối bị từ chối",
            "Máy chủ từ chối kết nối; không có dịch vụ SSH nào lắng nghe trên cổng đó."
        ),
        [CheckAddress, Retry, ContactAdmin]
    ),
    row!(
        NetUnreachable,
        "TMX-NET-003",
        Transient,
        ("Host unreachable", "There is no network route to the host."),
        (
            "Không thể truy cập máy chủ",
            "Không có đường mạng đến máy chủ."
        ),
        [CheckNetwork, CheckAddress, Retry]
    ),
    row!(
        NetLost,
        "TMX-NET-004",
        Transient,
        (
            "Connection lost",
            "The connection to the server was interrupted."
        ),
        ("Mất kết nối", "Kết nối đến máy chủ bị gián đoạn."),
        [Retry, CheckNetwork]
    ),
    row!(
        NetKeepalive,
        "TMX-NET-005",
        Transient,
        (
            "Server not responding",
            "The server stopped answering keepalive messages."
        ),
        (
            "Máy chủ không phản hồi",
            "Máy chủ đã ngừng trả lời các gói keepalive."
        ),
        [Retry, CheckNetwork]
    ),
    row!(
        NetServerClosed,
        "TMX-NET-006",
        Transient,
        (
            "Disconnected by server",
            "The server closed the connection."
        ),
        ("Máy chủ đã ngắt kết nối", "Máy chủ đã đóng kết nối."),
        [Retry, ContactAdmin]
    ),
    row!(
        DnsTimeout,
        "TMX-DNS-001",
        Transient,
        (
            "Name lookup timed out",
            "The DNS resolver did not answer in time."
        ),
        (
            "Hết thời gian phân giải tên",
            "Trình phân giải DNS không phản hồi kịp thời."
        ),
        [CheckNetwork, Retry]
    ),
    row!(
        DnsNotFound,
        "TMX-DNS-002",
        Transient,
        (
            "Host name not found",
            "The host name could not be resolved to an address."
        ),
        (
            "Không tìm thấy tên máy chủ",
            "Không thể phân giải tên máy chủ thành địa chỉ."
        ),
        [CheckAddress, CheckNetwork]
    ),
    row!(
        HostKeyChanged,
        "TMX-HOSTKEY-001",
        Never,
        (
            "Host key changed",
            "The server presented a different key than the trusted one. This can mean an attack; the connection was stopped."
        ),
        (
            "Khóa máy chủ đã thay đổi",
            "Máy chủ đưa ra khóa khác với khóa đã tin cậy. Đây có thể là một cuộc tấn công; kết nối đã bị dừng."
        ),
        [ReviewHostKey, ContactAdmin]
    ),
    row!(
        HostKeyRevoked,
        "TMX-HOSTKEY-002",
        Never,
        (
            "Host key revoked",
            "The server presented a key that has been revoked."
        ),
        (
            "Khóa máy chủ đã bị thu hồi",
            "Máy chủ đưa ra một khóa đã bị thu hồi."
        ),
        [ReviewHostKey, ContactAdmin]
    ),
    row!(
        HostKeyRejected,
        "TMX-HOSTKEY-003",
        Never,
        ("Host key not trusted", "The unknown host key was rejected."),
        (
            "Khóa máy chủ không được tin cậy",
            "Khóa máy chủ chưa biết đã bị từ chối."
        ),
        [ReviewHostKey]
    ),
    row!(
        HostKeyStore,
        "TMX-HOSTKEY-004",
        Never,
        (
            "Known hosts unavailable",
            "The trusted host keys could not be read or saved, so the server could not be verified."
        ),
        (
            "Không đọc được danh sách máy chủ tin cậy",
            "Không thể đọc hoặc lưu các khóa máy chủ tin cậy nên không xác minh được máy chủ."
        ),
        [Retry, EditSettings]
    ),
    row!(
        AuthFailed,
        "TMX-AUTH-001",
        Never,
        (
            "Authentication failed",
            "The server rejected every configured sign-in method."
        ),
        (
            "Xác thực thất bại",
            "Máy chủ từ chối mọi phương thức đăng nhập đã cấu hình."
        ),
        [CheckCredentials, EditSettings]
    ),
    row!(
        AuthTrivial,
        "TMX-AUTH-002",
        Never,
        (
            "Server skipped authentication",
            "The server accepted the login without checking any credential, which an impostor could do; the connection was refused."
        ),
        (
            "Máy chủ bỏ qua xác thực",
            "Máy chủ chấp nhận đăng nhập mà không kiểm tra thông tin xác thực nào, điều mà kẻ giả mạo có thể làm; kết nối đã bị từ chối."
        ),
        [ContactAdmin, EditSettings]
    ),
    row!(
        AuthKeyLoad,
        "TMX-AUTH-003",
        Never,
        (
            "Key cannot be used",
            "A private key could not be read or decrypted."
        ),
        (
            "Không dùng được khóa",
            "Không thể đọc hoặc giải mã khóa riêng tư."
        ),
        [CheckCredentials, EditSettings]
    ),
    row!(
        AuthCancelled,
        "TMX-AUTH-004",
        Never,
        ("Sign-in cancelled", "A required prompt was cancelled."),
        ("Đã hủy đăng nhập", "Một lời nhắc bắt buộc đã bị hủy."),
        [Retry]
    ),
    row!(
        SshProtocol,
        "TMX-SSH-001",
        Transient,
        (
            "SSH protocol error",
            "The SSH connection failed because of a protocol or transport error."
        ),
        (
            "Lỗi giao thức SSH",
            "Kết nối SSH thất bại do lỗi giao thức hoặc lỗi truyền tải."
        ),
        [Retry, ContactAdmin]
    ),
    row!(
        SshChannelRefused,
        "TMX-SSH-002",
        Never,
        (
            "Server refused the session",
            "The server would not open a terminal or channel, for example because of a session limit."
        ),
        (
            "Máy chủ từ chối phiên",
            "Máy chủ không mở terminal hoặc kênh, ví dụ do giới hạn số phiên."
        ),
        [ContactAdmin, Retry]
    ),
    row!(
        SshUnsupported,
        "TMX-SSH-003",
        Never,
        (
            "Not supported",
            "The operation is not supported on this connection."
        ),
        (
            "Không được hỗ trợ",
            "Thao tác này không được hỗ trợ trên kết nối này."
        ),
        [EditSettings]
    ),
    row!(
        SshUnexpected,
        "TMX-SSH-004",
        Never,
        (
            "Unexpected SSH error",
            "The SSH layer reported an error Termax does not recognise."
        ),
        (
            "Lỗi SSH không xác định",
            "Lớp SSH báo một lỗi mà Termax không nhận diện được."
        ),
        [Retry, ContactAdmin]
    ),
    row!(
        ConfigInvalid,
        "TMX-CFG-001",
        Never,
        (
            "Invalid connection settings",
            "The connection settings are incomplete or out of range."
        ),
        (
            "Cài đặt kết nối không hợp lệ",
            "Cài đặt kết nối chưa đầy đủ hoặc nằm ngoài phạm vi cho phép."
        ),
        [EditSettings]
    ),
    row!(
        ForwardBind,
        "TMX-FWD-001",
        Never,
        (
            "Port forward cannot listen",
            "The local port for a forward could not be opened, for example because it is in use."
        ),
        (
            "Không mở được cổng chuyển tiếp",
            "Không thể mở cổng cục bộ cho chuyển tiếp, ví dụ vì cổng đang được dùng."
        ),
        [EditSettings]
    ),
];

/// Why a session is disconnected: a stable code plus technical detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disconnect {
    /// Stable code.
    pub code: ErrorCode,
    /// Technical detail for the connection log and "details" view (never
    /// secrets or terminal content).
    pub detail: String,
}

impl Disconnect {
    /// A disconnect with `code` and `detail`.
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    /// Localised title and message.
    pub fn text(&self, lang: Lang) -> Text {
        self.code.entry().text(lang)
    }

    /// Suggested actions.
    pub fn actions(&self) -> &'static [SuggestedAction] {
        self.code.entry().actions
    }

    /// Whether automatic reconnect may follow.
    pub fn is_transient(&self) -> bool {
        self.code.retry() == Retry::Transient
    }
}

impl fmt::Display for Disconnect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}: {}",
            self.code,
            self.code.entry().en.title,
            self.detail
        )
    }
}

/// Code for a socket-level failure.
pub fn code_for_io(e: &io::Error) -> ErrorCode {
    match e.kind() {
        io::ErrorKind::ConnectionRefused => ErrorCode::NetRefused,
        io::ErrorKind::TimedOut => ErrorCode::NetTimeout,
        io::ErrorKind::HostUnreachable | io::ErrorKind::NetworkUnreachable => {
            ErrorCode::NetUnreachable
        }
        _ => ErrorCode::NetLost,
    }
}

/// Code for a connection that closed after being established. `None` for a
/// local close, which is not an error.
pub fn code_for_close(reason: &CloseReason) -> Option<ErrorCode> {
    match reason {
        CloseReason::KeepaliveTimeout => Some(ErrorCode::NetKeepalive),
        CloseReason::ServerDisconnect { .. } => Some(ErrorCode::NetServerClosed),
        CloseReason::Error(_) => Some(ErrorCode::NetLost),
        CloseReason::Local => None,
    }
}

/// Map an SSH facade error to a disconnect.
pub fn from_ssh(e: &SshError) -> Disconnect {
    let code = match e {
        SshError::InvalidConfig(_) => ErrorCode::ConfigInvalid,
        SshError::Connect { .. } => ErrorCode::NetUnreachable,
        SshError::Timeout { .. } => ErrorCode::NetTimeout,
        SshError::HostKeyRejected { .. } => ErrorCode::HostKeyRejected,
        SshError::HostKeyChanged { .. } => ErrorCode::HostKeyChanged,
        SshError::HostKeyRevoked { .. } => ErrorCode::HostKeyRevoked,
        SshError::HostKeyStore(_) => ErrorCode::HostKeyStore,
        SshError::AuthFailed { .. } => ErrorCode::AuthFailed,
        SshError::TrivialAuthRefused { .. } => ErrorCode::AuthTrivial,
        SshError::Cancelled => ErrorCode::AuthCancelled,
        SshError::KeyLoad { .. } => ErrorCode::AuthKeyLoad,
        SshError::ForwardBind { .. } => ErrorCode::ForwardBind,
        SshError::ChannelOpen(_) | SshError::ChannelRefused(_) => ErrorCode::SshChannelRefused,
        SshError::Unsupported(_) => ErrorCode::SshUnsupported,
        SshError::Closed(r) => code_for_close(r).unwrap_or(ErrorCode::NetLost),
        SshError::KeepaliveTimeout => ErrorCode::NetKeepalive,
        SshError::Protocol(_) => ErrorCode::SshProtocol,
        SshError::Io(io) => code_for_io(io),
        // A new facade variant must get its own code; until then it is never
        // retried (fail closed).
        _ => ErrorCode::SshUnexpected,
    };
    Disconnect::new(code, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::Path;

    fn doc_page() -> String {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        // Windows checkouts may convert line endings to CRLF.
        std::fs::read_to_string(root.join(DOC_PAGE))
            .expect("docs/error-codes.md exists")
            .replace("\r\n", "\n")
    }

    /// The section of the doc page under `## <id>`, up to the next heading.
    fn section<'a>(page: &'a str, id: &str) -> Option<&'a str> {
        let start = page.find(&format!("\n## {id}\n"))? + id.len() + 5;
        let rest = &page[start..];
        Some(&rest[..rest.find("\n## ").unwrap_or(rest.len())])
    }

    /// Verifies: FR-CO01-003 acceptance 2
    #[test]
    fn every_code_has_vi_and_en_text_and_a_doc_section() {
        let page = doc_page();
        let mut ids = HashSet::new();
        for e in CATALOGUE {
            assert!(ids.insert(e.id), "duplicate id {}", e.id);
            assert_eq!(e.code.as_str(), e.id);
            assert!(
                e.id.starts_with("TMX-") && e.id.rsplit('-').next().is_some_and(|n| n.len() == 3),
                "{} is not TMX-<AREA>-NNN",
                e.id
            );
            for t in [e.en, e.vi] {
                assert!(!t.title.is_empty() && !t.message.is_empty(), "{}", e.id);
            }
            assert_ne!(e.en.message, e.vi.message, "{} VI text is a copy", e.id);
            assert!(!e.actions.is_empty(), "{} has no suggested action", e.id);

            let link = e.code.doc_link();
            assert_eq!(
                link,
                format!("docs/error-codes.md#{}", e.id.to_ascii_lowercase())
            );
            let sec = section(&page, e.id).unwrap_or_else(|| panic!("no doc section for {}", e.id));
            assert!(
                sec.contains(&format!("**EN:** {}", e.en.title)),
                "{} EN doc",
                e.id
            );
            assert!(
                sec.contains(&format!("**VI:** {}", e.vi.title)),
                "{} VI doc",
                e.id
            );
        }
        // Every documented code is in the catalogue (no stale pages).
        // Control-endpoint codes (`TMX-CTL-*`) are catalogued and checked
        // in termax-control.
        for line in page
            .lines()
            .filter_map(|l| l.strip_prefix("## TMX-"))
            .filter(|l| !l.starts_with("CTL-"))
        {
            assert!(
                ids.contains(format!("TMX-{line}").as_str()),
                "stale doc TMX-{line}"
            );
        }
    }

    #[test]
    fn host_key_and_auth_failures_are_never_retried() {
        for e in CATALOGUE {
            let area = e.id.split('-').nth(1).unwrap_or_default();
            if matches!(area, "HOSTKEY" | "AUTH" | "CFG") {
                assert_eq!(e.retry, Retry::Never, "{}", e.id);
            }
            if area == "NET" {
                assert_eq!(e.retry, Retry::Transient, "{}", e.id);
            }
        }
        let changed = SshError::HostKeyChanged {
            host_port: "h:22".into(),
            old_key_type: "ssh-ed25519".into(),
            old_fingerprint: "SHA256:a".into(),
            new_key_type: "ssh-ed25519".into(),
            new_fingerprint: "SHA256:b".into(),
        };
        let d = from_ssh(&changed);
        assert_eq!(d.code, ErrorCode::HostKeyChanged);
        assert!(!d.is_transient());
        assert!(d.actions().contains(&SuggestedAction::ReviewHostKey));
        let auth = SshError::AuthFailed {
            user: "u".into(),
            server_methods: vec![],
            attempts: vec![],
        };
        assert!(!from_ssh(&auth).is_transient());
    }

    #[test]
    fn transport_failures_map_to_network_codes() {
        let refused = io::Error::from(io::ErrorKind::ConnectionRefused);
        assert_eq!(code_for_io(&refused), ErrorCode::NetRefused);
        assert_eq!(from_ssh(&SshError::Io(refused)).code, ErrorCode::NetRefused);
        assert_eq!(
            from_ssh(&SshError::KeepaliveTimeout).code,
            ErrorCode::NetKeepalive
        );
        assert_eq!(
            code_for_close(&CloseReason::ServerDisconnect {
                message: String::new()
            }),
            Some(ErrorCode::NetServerClosed)
        );
        assert_eq!(code_for_close(&CloseReason::Local), None);
        assert!(from_ssh(&SshError::Protocol("x".into())).is_transient());
    }
}
