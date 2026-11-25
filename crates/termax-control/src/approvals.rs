//! Requests that need the user's decision: pairing a new client, and
//! one-off operate actions from clients that lack the scope or that the
//! policy sends to Ask. The UI (or `tmx headless` on its terminal) is the
//! only party that decides; nothing is ever approved automatically.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, oneshot};

use crate::clients::{ClientKind, Scope};

/// Approval request id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ApprovalId(pub uuid::Uuid);

impl std::fmt::Display for ApprovalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// What is being asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalKind {
    /// A new client wants a token (SCR-04-08).
    Pairing {
        /// Name the client gave.
        name: String,
        /// Kind it claims.
        kind: ClientKind,
        /// Its process id, when the OS reports it.
        pid: Option<u32>,
        /// Its executable, when known.
        binary_path: Option<String>,
        /// Requested scopes.
        scopes: Vec<Scope>,
    },
    /// A paired client asks to perform one operate action.
    Operate {
        /// Client id.
        client_id: String,
        /// Client name.
        client_name: String,
        /// Method, e.g. `session.send_input`.
        method: String,
        /// Target session.
        session: String,
        /// What would happen, e.g. the input text or `signal INT`.
        summary: String,
        /// Why approval is needed (missing scope or a policy reason).
        reason: String,
    },
}

/// A pending request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApprovalRequest {
    /// Id to pass to [`Approvals::approve`] or [`Approvals::deny`].
    pub id: ApprovalId,
    /// The request.
    pub kind: ApprovalKind,
    /// When it was created.
    pub created_at: SystemTime,
}

/// The user's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Approved; for pairing, with these scopes (`None`: as requested).
    Approve(Option<Vec<Scope>>),
    /// Denied.
    Deny,
}

type Pending = BTreeMap<ApprovalId, (ApprovalRequest, oneshot::Sender<Decision>)>;

/// The approval queue.
#[derive(Debug)]
pub struct Approvals {
    pending: Mutex<Pending>,
    created: broadcast::Sender<ApprovalRequest>,
}

impl Default for Approvals {
    fn default() -> Self {
        Self {
            pending: Mutex::default(),
            created: broadcast::Sender::new(64),
        }
    }
}

impl Approvals {
    fn lock(&self) -> MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Queue a request; the receiver resolves when the user decides (or
    /// errors when the request is withdrawn).
    pub(crate) fn request(
        &self,
        kind: ApprovalKind,
    ) -> (ApprovalRequest, oneshot::Receiver<Decision>) {
        let req = ApprovalRequest {
            id: ApprovalId(uuid::Uuid::new_v4()),
            kind,
            created_at: SystemTime::now(),
        };
        let (tx, rx) = oneshot::channel();
        self.lock().insert(req.id, (req.clone(), tx));
        // Nobody listening yet is fine; `pending` still lists it.
        let _ = self.created.send(req.clone());
        (req, rx)
    }

    /// Withdraw a request nobody answered (timeout, client gone).
    pub(crate) fn withdraw(&self, id: ApprovalId) {
        self.lock().remove(&id);
    }

    /// New requests from now on (the UI's approval sheet listens here).
    pub fn subscribe(&self) -> broadcast::Receiver<ApprovalRequest> {
        self.created.subscribe()
    }

    /// Requests waiting for a decision, oldest first.
    pub fn pending(&self) -> Vec<ApprovalRequest> {
        let mut v: Vec<_> = self.lock().values().map(|(r, _)| r.clone()).collect();
        v.sort_by_key(|r| r.created_at);
        v
    }

    /// Approve a request (pairing: with the requested scopes). Returns
    /// whether it was still pending.
    pub fn approve(&self, id: ApprovalId) -> bool {
        self.decide(id, Decision::Approve(None))
    }

    /// Approve a pairing with fewer scopes than requested.
    pub fn approve_with_scopes(&self, id: ApprovalId, scopes: Vec<Scope>) -> bool {
        self.decide(id, Decision::Approve(Some(scopes)))
    }

    /// Deny a request.
    pub fn deny(&self, id: ApprovalId) -> bool {
        self.decide(id, Decision::Deny)
    }

    fn decide(&self, id: ApprovalId, decision: Decision) -> bool {
        match self.lock().remove(&id) {
            Some((_, tx)) => tx.send(decision).is_ok(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairing() -> ApprovalKind {
        ApprovalKind::Pairing {
            name: "tmx".into(),
            kind: ClientKind::Cli,
            pid: Some(1),
            binary_path: None,
            scopes: vec![Scope::Observe],
        }
    }

    #[tokio::test]
    async fn requests_are_announced_listed_and_decided_once() {
        let a = Approvals::default();
        let mut created = a.subscribe();
        let (req, rx) = a.request(pairing());
        assert_eq!(created.recv().await.unwrap(), req);
        assert_eq!(a.pending(), vec![req.clone()]);
        assert!(a.approve_with_scopes(req.id, vec![]));
        assert_eq!(rx.await.unwrap(), Decision::Approve(Some(vec![])));
        assert!(a.pending().is_empty());
        assert!(!a.deny(req.id), "already decided");
    }

    #[tokio::test]
    async fn withdrawn_requests_cannot_be_approved() {
        let a = Approvals::default();
        let (req, rx) = a.request(pairing());
        a.withdraw(req.id);
        assert!(!a.approve(req.id));
        assert!(rx.await.is_err());
    }
}
