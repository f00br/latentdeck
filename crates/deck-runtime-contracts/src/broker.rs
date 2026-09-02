use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::compatibility::{ContractId, ContractValidationError, PackageIdentity};

pub const MAX_WARM_SESSIONS: usize = 4;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(ContractId);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ContractValidationError> {
                ContractId::new(value).map(Self)
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

string_id!(SessionId);
string_id!(WorkerId);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WarmSession {
    pub session_id: SessionId,
    pub worker_id: WorkerId,
    pub deck: PackageIdentity,
    pub codec: PackageIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForegroundLease {
    pub session_id: SessionId,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputPinKind {
    Capture,
    Mp4,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputPinToken {
    session_id: SessionId,
    lease_generation: u64,
    pin_generation: u64,
    kind: OutputPinKind,
}

impl OutputPinToken {
    #[must_use]
    pub const fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn lease_generation(&self) -> u64 {
        self.lease_generation
    }

    #[must_use]
    pub const fn pin_generation(&self) -> u64 {
        self.pin_generation
    }

    #[must_use]
    pub const fn kind(&self) -> OutputPinKind {
        self.kind
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSnapshot {
    pub sessions: Vec<WarmSession>,
    pub foreground_output: Option<ForegroundLease>,
    pub output_pin: Option<OutputPinToken>,
}

#[derive(Clone, Debug, Default)]
pub struct SessionBroker {
    sessions: BTreeMap<SessionId, WarmSession>,
    foreground_output: Option<ForegroundLease>,
    output_pin: Option<OutputPinToken>,
    next_lease_generation: u64,
    next_pin_generation: u64,
}

impl SessionBroker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    #[must_use]
    pub fn contains_session(&self, session_id: &SessionId) -> bool {
        self.sessions.contains_key(session_id)
    }

    #[must_use]
    pub fn foreground_output(&self) -> Option<&ForegroundLease> {
        self.foreground_output.as_ref()
    }

    #[must_use]
    pub fn output_pin(&self) -> Option<&OutputPinToken> {
        self.output_pin.as_ref()
    }

    #[must_use]
    pub fn sessions(&self) -> impl ExactSizeIterator<Item = &WarmSession> {
        self.sessions.values()
    }

    #[must_use]
    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            sessions: self.sessions.values().cloned().collect(),
            foreground_output: self.foreground_output.clone(),
            output_pin: self.output_pin.clone(),
        }
    }

    pub fn open_session(&mut self, session: WarmSession) -> Result<(), BrokerError> {
        if self.sessions.contains_key(&session.session_id) {
            return Err(BrokerError::SessionAlreadyExists);
        }
        if self
            .sessions
            .values()
            .any(|existing| existing.worker_id == session.worker_id)
        {
            return Err(BrokerError::WorkerAlreadyAssigned);
        }
        if self.sessions.len() == MAX_WARM_SESSIONS {
            return Err(BrokerError::SessionCapacityExceeded);
        }
        self.sessions.insert(session.session_id.clone(), session);
        Ok(())
    }

    pub fn close_session(&mut self, session_id: &SessionId) -> Result<WarmSession, BrokerError> {
        if !self.sessions.contains_key(session_id) {
            return Err(BrokerError::SessionNotFound);
        }
        if self
            .output_pin
            .as_ref()
            .is_some_and(|pin| &pin.session_id == session_id)
        {
            return Err(BrokerError::SessionOutputLeasePinned);
        }

        let removed = self
            .sessions
            .remove(session_id)
            .ok_or(BrokerError::SessionNotFound)?;
        if self
            .foreground_output
            .as_ref()
            .is_some_and(|lease| &lease.session_id == session_id)
        {
            self.foreground_output = None;
        }
        Ok(removed)
    }

    pub fn handle_worker_fault(
        &mut self,
        worker_id: &WorkerId,
    ) -> Result<WarmSession, BrokerError> {
        let session_id = self
            .sessions
            .values()
            .find(|session| &session.worker_id == worker_id)
            .map(|session| session.session_id.clone())
            .ok_or(BrokerError::WorkerNotFound)?;
        let removed = self
            .sessions
            .remove(&session_id)
            .ok_or(BrokerError::SessionNotFound)?;
        if self
            .foreground_output
            .as_ref()
            .is_some_and(|lease| lease.session_id == session_id)
        {
            self.foreground_output = None;
            self.output_pin = None;
        }
        Ok(removed)
    }

    pub fn switch_foreground(
        &mut self,
        session_id: &SessionId,
    ) -> Result<ForegroundLease, BrokerError> {
        if !self.sessions.contains_key(session_id) {
            return Err(BrokerError::SessionNotFound);
        }
        if let Some(current) = &self.foreground_output
            && &current.session_id == session_id
        {
            return Ok(current.clone());
        }
        if self.output_pin.is_some() {
            return Err(BrokerError::SessionOutputLeasePinned);
        }

        self.next_lease_generation = self
            .next_lease_generation
            .checked_add(1)
            .ok_or(BrokerError::LeaseGenerationExhausted)?;
        let lease = ForegroundLease {
            session_id: session_id.clone(),
            generation: self.next_lease_generation,
        };
        self.foreground_output = Some(lease.clone());
        Ok(lease)
    }

    pub fn clear_foreground(&mut self) -> Result<Option<ForegroundLease>, BrokerError> {
        if self.output_pin.is_some() {
            return Err(BrokerError::SessionOutputLeasePinned);
        }
        Ok(self.foreground_output.take())
    }

    pub fn pin_foreground(
        &mut self,
        session_id: &SessionId,
        kind: OutputPinKind,
    ) -> Result<OutputPinToken, BrokerError> {
        let lease = self
            .foreground_output
            .as_ref()
            .filter(|lease| &lease.session_id == session_id)
            .ok_or(BrokerError::SessionDoesNotOwnForeground)?;
        if self.output_pin.is_some() {
            return Err(BrokerError::SessionOutputLeasePinned);
        }
        self.next_pin_generation = self
            .next_pin_generation
            .checked_add(1)
            .ok_or(BrokerError::PinGenerationExhausted)?;
        let token = OutputPinToken {
            session_id: session_id.clone(),
            lease_generation: lease.generation,
            pin_generation: self.next_pin_generation,
            kind,
        };
        self.output_pin = Some(token.clone());
        Ok(token)
    }

    pub fn release_output_pin(&mut self, token: &OutputPinToken) -> Result<(), BrokerError> {
        if self.output_pin.as_ref() != Some(token) {
            return Err(BrokerError::OutputPinMismatch);
        }
        self.output_pin = None;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrokerError {
    #[error("session.capacity_exceeded")]
    #[serde(rename = "session.capacity_exceeded")]
    SessionCapacityExceeded,
    #[error("session.already_exists")]
    #[serde(rename = "session.already_exists")]
    SessionAlreadyExists,
    #[error("session.worker_already_assigned")]
    #[serde(rename = "session.worker_already_assigned")]
    WorkerAlreadyAssigned,
    #[error("session.not_found")]
    #[serde(rename = "session.not_found")]
    SessionNotFound,
    #[error("session.worker_not_found")]
    #[serde(rename = "session.worker_not_found")]
    WorkerNotFound,
    #[error("session.output_lease_pinned")]
    #[serde(rename = "session.output_lease_pinned")]
    SessionOutputLeasePinned,
    #[error("session.output_lease_not_owned")]
    #[serde(rename = "session.output_lease_not_owned")]
    SessionDoesNotOwnForeground,
    #[error("session.output_pin_mismatch")]
    #[serde(rename = "session.output_pin_mismatch")]
    OutputPinMismatch,
    #[error("session.lease_generation_exhausted")]
    #[serde(rename = "session.lease_generation_exhausted")]
    LeaseGenerationExhausted,
    #[error("session.pin_generation_exhausted")]
    #[serde(rename = "session.pin_generation_exhausted")]
    PinGenerationExhausted,
}

impl BrokerError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::SessionCapacityExceeded => "session.capacity_exceeded",
            Self::SessionAlreadyExists => "session.already_exists",
            Self::WorkerAlreadyAssigned => "session.worker_already_assigned",
            Self::SessionNotFound => "session.not_found",
            Self::WorkerNotFound => "session.worker_not_found",
            Self::SessionOutputLeasePinned => "session.output_lease_pinned",
            Self::SessionDoesNotOwnForeground => "session.output_lease_not_owned",
            Self::OutputPinMismatch => "session.output_pin_mismatch",
            Self::LeaseGenerationExhausted => "session.lease_generation_exhausted",
            Self::PinGenerationExhausted => "session.pin_generation_exhausted",
        }
    }
}
