use thiserror::Error;

/// Every fallible path in the suite returns `BotResult<T>`.
pub type BotResult<T> = Result<T, BotError>;

#[derive(Debug, Error)]
pub enum BotError {
    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("http error: {0}")]
    Http(String),

    #[error("websocket error: {0}")]
    WebSocket(String),

    #[error("rpc error: {0}")]
    Rpc(String),

    #[error("solana error: {0}")]
    Solana(String),

    #[error("invalid pubkey: {0}")]
    InvalidPubkey(String),

    #[error("encoding error: {0}")]
    Encoding(String),

    #[error("signing error: {0}")]
    Signing(String),

    #[error("signer error: {0}")]
    Signer(#[from] SignerError),

    #[error("risk rejected: {0}")]
    RiskRejected(String),

    #[error("kill switch engaged: {0}")]
    KillSwitch(String),

    #[error("module disabled: {0}")]
    ModuleDisabled(String),

    #[error("insufficient balance: need {needed} have {have} {symbol}")]
    InsufficientBalance {
        needed: f64,
        have: f64,
        symbol: String,
    },

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("timeout: {0}")]
    Timeout(String),

    #[error("database error: {0}")]
    Db(String),

    /// A distributed execution claim was not granted or was lost: another
    /// replica owns the execution, or this replica's lease was fenced
    /// (Prompt 3 §E/§G). Money-moving work must stop on this error.
    #[error("execution claim rejected: {0}")]
    ClaimRejected(String),

    /// The ownership store could not be reached. Money-moving operations
    /// FAIL CLOSED on this error — "could not acquire ownership" is never
    /// "ownership acquired" (Prompt 3 §K).
    #[error("ownership store unavailable: {0}")]
    OwnershipUnavailable(String),

    #[error("{0}")]
    Other(String),
}

impl BotError {
    pub fn other<S: Into<String>>(msg: S) -> Self {
        BotError::Other(msg.into())
    }

    pub fn config<S: Into<String>>(msg: S) -> Self {
        BotError::Config(msg.into())
    }

    pub fn claim_rejected<S: Into<String>>(msg: S) -> Self {
        BotError::ClaimRejected(msg.into())
    }

    pub fn ownership_unavailable<S: Into<String>>(msg: S) -> Self {
        BotError::OwnershipUnavailable(msg.into())
    }

    pub fn db<S: Into<String>>(msg: S) -> Self {
        BotError::Db(msg.into())
    }

    pub fn rpc<S: Into<String>>(msg: S) -> Self {
        BotError::Rpc(msg.into())
    }

    pub fn solana<S: Into<String>>(msg: S) -> Self {
        BotError::Solana(msg.into())
    }

    pub fn http<S: Into<String>>(msg: S) -> Self {
        BotError::Http(msg.into())
    }

    pub fn ws<S: Into<String>>(msg: S) -> Self {
        BotError::WebSocket(msg.into())
    }

    pub fn signing<S: Into<String>>(msg: S) -> Self {
        BotError::Signing(msg.into())
    }

    pub fn encoding<S: Into<String>>(msg: S) -> Self {
        BotError::Encoding(msg.into())
    }

    pub fn risk<S: Into<String>>(msg: S) -> Self {
        BotError::RiskRejected(msg.into())
    }

    pub fn invalid<S: Into<String>>(msg: S) -> Self {
        BotError::InvalidArgument(msg.into())
    }

    /// Errors worth paging a human about on Telegram.
    pub fn is_alertable(&self) -> bool {
        matches!(
            self,
            BotError::KillSwitch(_)
                | BotError::InsufficientBalance { .. }
                | BotError::Rpc(_)
                | BotError::Solana(_)
                | BotError::Signing(_)
                | BotError::Signer(_)
        )
    }

    /// Transient errors are worth retrying; permanent ones are not.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            BotError::Http(_)
                | BotError::WebSocket(_)
                | BotError::Rpc(_)
                | BotError::Timeout(_)
                | BotError::Io(_)
        )
    }
}

/// Structured signer/key-custody errors (see `solana_kit::signer`).
///
/// Every variant is deliberately secret-free: identities, public keys and
/// context strings only. Errors produced while loading or using key material
/// must never carry the material itself.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignerError {
    #[error("signer identity not found in registry: {identity}")]
    NotFound { identity: String },

    #[error("signer identity configured twice: {identity}")]
    DuplicateIdentity { identity: String },

    #[error("no signer available for required transaction signer {pubkey}")]
    MissingSigner { pubkey: String },

    #[error("requested extra signer {pubkey} is not required by the compiled message")]
    ExtraSignerNotRequired { pubkey: String },

    #[error("signer mismatch: expected {expected}, got {found}")]
    SignerMismatch { expected: String, found: String },

    #[error("signing failed ({context})")]
    SigningFailed { context: String },

    #[error("invalid signer: {reason}")]
    InvalidSigner { reason: String },

    #[error("signer backend not supported by this build: {provider}")]
    UnsupportedBackend { provider: String },

    #[error("failed to load signer secret ({context})")]
    SecretLoad { context: String },

    #[error("unsafe signing configuration: {reason}")]
    UnsafeConfiguration { reason: String },
}

impl SignerError {
    /// True when the failure is a configuration/custody problem rather than a
    /// transient signing outage — useful for fail-fast startup decisions.
    pub fn is_configuration(&self) -> bool {
        matches!(
            self,
            SignerError::NotFound { .. }
                | SignerError::DuplicateIdentity { .. }
                | SignerError::UnsupportedBackend { .. }
                | SignerError::SecretLoad { .. }
                | SignerError::UnsafeConfiguration { .. }
                | SignerError::InvalidSigner { .. }
        )
    }
}
