//! The policy engine: actor and host context, kill switch, rate limiting
//! and audit recording around rule evaluation.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oxt_core::Env;
use serde::{Deserialize, Serialize};

use crate::audit::AuditLog;
use crate::rules::{Effect, RuleSet, Verdict};

/// Who proposed a command.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Actor {
    /// The in-app assistant.
    InApp,
    /// An external MCP client.
    External {
        /// Client identifier.
        client_id: String,
    },
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Actor::InApp => f.write_str("in-app"),
            Actor::External { client_id } => write!(f, "external:{client_id}"),
        }
    }
}

/// What an external client may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalMode {
    /// May read context but never execute commands (the default).
    #[default]
    ObserveOnly,
    /// Commands are evaluated against the rules like in-app ones.
    Operate,
}

/// Everything needed to evaluate one proposed command.
#[derive(Debug, Clone, Copy)]
pub struct EvalRequest<'a> {
    /// Who proposed the command.
    pub actor: &'a Actor,
    /// The command line.
    pub command: &'a str,
    /// Environment of the target host.
    pub env: Env,
    /// Tags of the target host.
    pub tags: &'a [String],
}

impl EvalRequest<'_> {
    /// True when the host is production by env or by a `prod` tag.
    pub fn is_prod(&self) -> bool {
        self.env == Env::Prod
            || self
                .tags
                .iter()
                .any(|t| t.parse::<Env>().is_ok_and(|e| e == Env::Prod))
    }
}

/// Source of wall-clock time, injectable for tests.
pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> u64;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }
}

/// A manually advanced clock for deterministic tests.
#[derive(Debug, Clone, Default)]
pub struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    /// A clock reading `start_ms`.
    pub fn new(start_ms: u64) -> Self {
        Self(Arc::new(AtomicU64::new(start_ms)))
    }

    /// Move time forward.
    pub fn advance(&self, by: Duration) {
        let ms = u64::try_from(by.as_millis()).unwrap_or(u64::MAX);
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// A shared emergency stop. Clones share state; while engaged every
/// evaluation is denied.
#[derive(Debug, Clone, Default)]
pub struct KillSwitch(Arc<AtomicBool>);

impl KillSwitch {
    /// Deny everything from now on.
    pub fn engage(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Resume normal evaluation.
    pub fn disengage(&self) {
        self.0.store(false, Ordering::SeqCst);
    }

    /// Whether the switch is engaged.
    pub fn is_engaged(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Per-actor token bucket: up to `max_commands` per `window`, refilled
/// continuously.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    /// Bucket capacity (burst size and commands per window).
    pub max_commands: u32,
    /// Time to refill a full bucket.
    pub window: Duration,
}

impl Default for RateLimit {
    fn default() -> Self {
        Self {
            max_commands: 30,
            window: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    updated_ms: u64,
}

/// Engine configuration.
#[derive(Debug, Clone, Default)]
pub struct PolicyConfig {
    /// User rules layered on top of the built-in rules.
    pub rules: RuleSet,
    /// Per-actor rate limit; `None` disables limiting.
    pub rate_limit: Option<RateLimit>,
}

/// Decides allow / ask / deny for agent-proposed commands and records
/// every decision in a hash-chained audit log. Safe to share across
/// threads.
pub struct PolicyEngine {
    rules: RuleSet,
    rate_limit: Option<RateLimit>,
    buckets: Mutex<HashMap<String, Bucket>>,
    kill_switch: KillSwitch,
    operators: RwLock<HashSet<String>>,
    clock: Arc<dyn Clock>,
    audit: Mutex<AuditLog>,
}

impl fmt::Debug for PolicyEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PolicyEngine")
            .field("rules", &self.rules.rules().len())
            .field("rate_limit", &self.rate_limit)
            .field("kill_switch", &self.kill_switch.is_engaged())
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // The guarded data stays consistent even if a holder panicked: every
    // critical section is a single insert or update.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl PolicyEngine {
    /// An engine using the system clock.
    pub fn new(config: PolicyConfig) -> Self {
        Self::with_clock(config, Arc::new(SystemClock))
    }

    /// An engine using the given clock.
    pub fn with_clock(config: PolicyConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            rules: config.rules,
            rate_limit: config.rate_limit,
            buckets: Mutex::new(HashMap::new()),
            kill_switch: KillSwitch::default(),
            operators: RwLock::new(HashSet::new()),
            clock,
            audit: Mutex::new(AuditLog::new()),
        }
    }

    /// A handle to this engine's kill switch.
    pub fn kill_switch(&self) -> KillSwitch {
        self.kill_switch.clone()
    }

    /// Set what an external client may do.
    pub fn set_external_mode(&self, client_id: &str, mode: ExternalMode) {
        let mut ops = self.operators.write().unwrap_or_else(|e| e.into_inner());
        match mode {
            ExternalMode::Operate => ops.insert(client_id.to_string()),
            ExternalMode::ObserveOnly => ops.remove(client_id),
        };
    }

    /// Current mode of an external client (observe-only unless granted).
    pub fn external_mode(&self, client_id: &str) -> ExternalMode {
        let ops = self.operators.read().unwrap_or_else(|e| e.into_inner());
        if ops.contains(client_id) {
            ExternalMode::Operate
        } else {
            ExternalMode::ObserveOnly
        }
    }

    /// Evaluate a proposed command and append the decision to the audit
    /// log. Checks, in order: kill switch, observe-only external actor,
    /// rate limit, then command rules; Allow on a production host is
    /// downgraded to Ask.
    pub fn evaluate(&self, req: &EvalRequest<'_>) -> Verdict {
        let now = self.clock.now_ms();
        let verdict = self.decide(req, now);
        lock(&self.audit).append(now, req.actor.clone(), req.command, &verdict);
        verdict
    }

    fn decide(&self, req: &EvalRequest<'_>, now: u64) -> Verdict {
        if self.kill_switch.is_engaged() {
            return Verdict::new(Effect::Deny, "kill switch engaged");
        }
        if let Actor::External { client_id } = req.actor
            && self.external_mode(client_id) == ExternalMode::ObserveOnly
        {
            return Verdict::new(Effect::Deny, "observe-only");
        }
        if let Some(limit) = self.rate_limit
            && !self.take_token(req.actor, limit, now)
        {
            return Verdict::new(
                Effect::Deny,
                format!(
                    "rate limit exceeded: at most {} commands per {}s",
                    limit.max_commands,
                    limit.window.as_secs_f64()
                ),
            );
        }
        let mut verdict = self.rules.evaluate(req.command);
        if verdict.effect == Effect::Allow && req.is_prod() {
            verdict = Verdict::new(Effect::Ask, "production host: approval required");
        }
        verdict
    }

    fn take_token(&self, actor: &Actor, limit: RateLimit, now: u64) -> bool {
        let capacity = f64::from(limit.max_commands);
        let window_ms = limit.window.as_millis().max(1) as f64;
        let mut buckets = lock(&self.buckets);
        let bucket = buckets.entry(actor.to_string()).or_insert(Bucket {
            tokens: capacity,
            updated_ms: now,
        });
        let elapsed = now.saturating_sub(bucket.updated_ms) as f64;
        bucket.tokens = (bucket.tokens + elapsed * capacity / window_ms).min(capacity);
        bucket.updated_ms = now.max(bucket.updated_ms);
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// A snapshot of the audit log.
    pub fn audit_log(&self) -> AuditLog {
        lock(&self.audit).clone()
    }
}
