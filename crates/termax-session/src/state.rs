//! Session lifecycle states and the transitions allowed between them.

use std::fmt;
use std::time::Duration;

use crate::codes::Disconnect;

/// Where a session is in its life.
///
/// `Idle → Resolving → Connecting → Handshaking → Authenticating → Open`,
/// then `Stalled`, `Disconnected(reason)`, `Reconnecting` and finally
/// `Closed`. A Duplicate that reuses an open connection goes `Idle → Open`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// Created, not started.
    Idle,
    /// Resolving the first hop's name.
    Resolving,
    /// Opening the TCP connection.
    Connecting,
    /// SSH version exchange, key exchange and host-key check.
    Handshaking,
    /// Host key accepted; authenticating.
    Authenticating,
    /// Shell running.
    Open,
    /// Keepalives are going unanswered.
    Stalled,
    /// Ended with a stable error code; may be followed by `Reconnecting`.
    Disconnected(Disconnect),
    /// Waiting before reconnect attempt `attempt`.
    Reconnecting {
        /// 1-based attempt number (shown in the overlay).
        attempt: u32,
        /// Attempt limit, `None` for unlimited.
        max_attempts: Option<u32>,
        /// Wait before this attempt, jitter included.
        delay: Duration,
    },
    /// Closed for good.
    Closed,
}

impl SessionState {
    /// The state without its data.
    pub fn kind(&self) -> StateKind {
        match self {
            SessionState::Idle => StateKind::Idle,
            SessionState::Resolving => StateKind::Resolving,
            SessionState::Connecting => StateKind::Connecting,
            SessionState::Handshaking => StateKind::Handshaking,
            SessionState::Authenticating => StateKind::Authenticating,
            SessionState::Open => StateKind::Open,
            SessionState::Stalled => StateKind::Stalled,
            SessionState::Disconnected(_) => StateKind::Disconnected,
            SessionState::Reconnecting { .. } => StateKind::Reconnecting,
            SessionState::Closed => StateKind::Closed,
        }
    }
}

impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionState::Disconnected(d) => write!(f, "Disconnected({})", d.code),
            SessionState::Reconnecting {
                attempt,
                max_attempts: Some(max),
                ..
            } => write!(f, "Reconnecting({attempt}/{max})"),
            SessionState::Reconnecting { attempt, .. } => write!(f, "Reconnecting({attempt})"),
            other => write!(f, "{:?}", other.kind()),
        }
    }
}

/// [`SessionState`] without data, for transition checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateKind {
    /// See [`SessionState::Idle`].
    Idle,
    /// See [`SessionState::Resolving`].
    Resolving,
    /// See [`SessionState::Connecting`].
    Connecting,
    /// See [`SessionState::Handshaking`].
    Handshaking,
    /// See [`SessionState::Authenticating`].
    Authenticating,
    /// See [`SessionState::Open`].
    Open,
    /// See [`SessionState::Stalled`].
    Stalled,
    /// See [`SessionState::Disconnected`].
    Disconnected,
    /// See [`SessionState::Reconnecting`].
    Reconnecting,
    /// See [`SessionState::Closed`].
    Closed,
}

impl StateKind {
    /// Every state.
    pub const ALL: [StateKind; 10] = [
        StateKind::Idle,
        StateKind::Resolving,
        StateKind::Connecting,
        StateKind::Handshaking,
        StateKind::Authenticating,
        StateKind::Open,
        StateKind::Stalled,
        StateKind::Disconnected,
        StateKind::Reconnecting,
        StateKind::Closed,
    ];

    /// Whether `self → next` is a legal transition. Every state except
    /// `Closed` may close; every connecting or open state may fail into
    /// `Disconnected`.
    pub fn can_transition_to(self, next: StateKind) -> bool {
        use StateKind::*;
        if self == Closed {
            return false;
        }
        if next == Closed {
            return true;
        }
        matches!(
            (self, next),
            (Idle, Resolving)
                | (Idle, Open)
                | (Resolving, Connecting)
                | (Connecting, Handshaking)
                | (Handshaking, Authenticating)
                | (Authenticating, Open)
                | (Open, Stalled)
                | (Stalled, Open)
                | (
                    Idle | Resolving | Connecting | Handshaking | Authenticating,
                    Disconnected
                )
                | (Open | Stalled | Reconnecting, Disconnected)
                | (Disconnected, Reconnecting)
                | (Reconnecting, Resolving)
        )
    }
}

/// A transition the state machine does not allow.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal session state transition {from:?} -> {to:?}")]
pub struct InvalidTransition {
    /// Current state.
    pub from: StateKind,
    /// Requested state.
    pub to: StateKind,
}

/// Check a transition.
pub fn check_transition(from: &SessionState, to: &SessionState) -> Result<(), InvalidTransition> {
    let (from, to) = (from.kind(), to.kind());
    if from.can_transition_to(to) {
        Ok(())
    } else {
        Err(InvalidTransition { from, to })
    }
}

#[cfg(test)]
mod tests {
    use super::StateKind::*;
    use super::*;

    const LEGAL: &[(StateKind, StateKind)] = &[
        (Idle, Resolving),
        (Idle, Open),
        (Idle, Disconnected),
        (Idle, Closed),
        (Resolving, Connecting),
        (Resolving, Disconnected),
        (Resolving, Closed),
        (Connecting, Handshaking),
        (Connecting, Disconnected),
        (Connecting, Closed),
        (Handshaking, Authenticating),
        (Handshaking, Disconnected),
        (Handshaking, Closed),
        (Authenticating, Open),
        (Authenticating, Disconnected),
        (Authenticating, Closed),
        (Open, Stalled),
        (Open, Disconnected),
        (Open, Closed),
        (Stalled, Open),
        (Stalled, Disconnected),
        (Stalled, Closed),
        (Disconnected, Reconnecting),
        (Disconnected, Closed),
        (Reconnecting, Resolving),
        (Reconnecting, Disconnected),
        (Reconnecting, Closed),
    ];

    /// Verifies: FR-CO01-003 specified behaviour
    #[test]
    fn every_legal_and_illegal_transition() {
        for from in StateKind::ALL {
            for to in StateKind::ALL {
                let expected = LEGAL.contains(&(from, to));
                assert_eq!(
                    from.can_transition_to(to),
                    expected,
                    "{from:?} -> {to:?} should be {}",
                    if expected { "legal" } else { "illegal" }
                );
            }
        }
        // Closed is terminal and nothing skips the handshake.
        assert!(!Closed.can_transition_to(Idle));
        assert!(!Resolving.can_transition_to(Open));
        assert!(!Disconnected.can_transition_to(Open));
    }

    #[test]
    fn check_transition_reports_the_kinds() {
        let d = SessionState::Disconnected(Disconnect::new(
            crate::codes::ErrorCode::NetRefused,
            "refused",
        ));
        assert!(check_transition(&SessionState::Connecting, &d).is_ok());
        assert_eq!(
            check_transition(&d, &SessionState::Open),
            Err(InvalidTransition {
                from: Disconnected,
                to: Open
            })
        );
        assert_eq!(d.to_string(), "Disconnected(TMX-NET-002)");
    }
}
