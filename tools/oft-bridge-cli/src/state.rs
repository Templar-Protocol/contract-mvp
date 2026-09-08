use std::collections::{BTreeMap, BTreeSet};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead as _, BufReader, Read as _, Write as _},
    path::{Component, Path, PathBuf},
};

#[cfg(unix)]
use std::os::fd::AsRawFd as _;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    canonical_sha256,
    domain::{
        ArtifactRefV1, DesiredRouteV1, Direction, Environment, MessageRecordV1, MessageStageV1,
        MessageStatusEventV1, RouteStateV1, Vm, SCHEMA_VERSION,
    },
    error::{Error, Result},
};

const STATE_FILE: &str = "route.json";
const OPERATIONS_FILE: &str = "operations.jsonl";
const MESSAGES_FILE: &str = "messages.jsonl";
const LOCK_FILE: &str = ".lock";
const AUTHORITY_LOCK_DIR: &str = ".authority";

fn valid_message_transition(
    direction: Direction,
    previous: MessageStageV1,
    next: MessageStageV1,
) -> bool {
    use MessageStageV1::*;
    next == Reobserved
        || matches!(
            (direction, previous, next),
            (
                Direction::StellarToEvm,
                ForwardSourceAccepted,
                ForwardLocked
            ) | (Direction::StellarToEvm, ForwardLocked, ForwardVerified)
                | (Direction::StellarToEvm, ForwardVerified, ForwardCommitted)
                | (Direction::StellarToEvm, ForwardCommitted, ForwardMinted)
                | (
                    Direction::EvmToStellar,
                    ReverseSourceAccepted,
                    ReverseBurned
                )
                | (Direction::EvmToStellar, ReverseBurned, ReverseVerified)
                | (Direction::EvmToStellar, ReverseVerified, ReverseCommitted)
                | (Direction::EvmToStellar, ReverseCommitted, ReverseUnlocked)
        )
}

#[derive(Debug)]
pub struct RouteLock {
    _file: File,
}

impl RouteLock {
    pub fn acquire(state_dir: &Path) -> Result<Self> {
        let path = state_dir.join(LOCK_FILE);
        let file = open_advisory_lock(&path)?;
        try_lock_exclusive(&file).map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                Error::Conflict(format!("route is busy: {}", state_dir.display()))
            } else {
                Error::Io(error)
            }
        })?;
        Ok(Self { _file: file })
    }
}

// The persistent file is only an inode for the kernel advisory lock. Closing
// `_file` releases the lock even if the process is killed, so there is no
// stale marker to delete or race another process to reclaim.
/// Non-authoritative Phase-A binding: the `{environment, vm, sender}`
/// authority-domain key plus the canonical route state path, each with a
/// digest, captured from a plain state-file read before any lock is taken.
/// It reserves no nonce and trusts no mutable decision; `acquire_mutation`
/// re-derives the same binding under both locks and rejects it if the
/// underlying state changed in between.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhaseABinding {
    route_canonical_path: PathBuf,
    environment: Environment,
    vm: Vm,
    sender: String,
    domain_sha256: String,
    route_path_sha256: String,
}

impl PhaseABinding {
    fn derive(route_path: &Path, environment: Environment, vm: Vm, sender: &str) -> Result<Self> {
        #[derive(Serialize)]
        struct DomainBindingKey<'a> {
            environment: Environment,
            vm: Vm,
            sender: &'a str,
        }
        let sender = canonical_authority_sender(vm, sender)?;
        let route_canonical_path = fs::canonicalize(route_path).map_err(|_| {
            Error::InvalidInput(format!(
                "phase-a binding requires an existing state directory: {}",
                route_path.display()
            ))
        })?;
        let domain_sha256 = canonical_sha256(&DomainBindingKey {
            environment,
            vm,
            sender: &sender,
        })?;
        let route_path_sha256 = route_path_sha256_digest(&route_canonical_path);
        Ok(Self {
            route_canonical_path,
            environment,
            vm,
            sender,
            domain_sha256,
            route_path_sha256,
        })
    }

    /// Canonical route state directory this binding was derived from.
    pub fn route_canonical_path(&self) -> &Path {
        &self.route_canonical_path
    }

    pub fn environment(&self) -> Environment {
        self.environment
    }

    pub fn vm(&self) -> Vm {
        self.vm
    }

    pub fn sender(&self) -> &str {
        &self.sender
    }

    /// Digest of the `{environment, vm, sender}` authority-domain key. The
    /// domain lock lives under the operation-store root at
    /// `.authority/<domain_sha256>.lock`, which is why different route files
    /// cannot bypass the sender-domain fence.
    pub fn domain_sha256(&self) -> &str {
        &self.domain_sha256
    }

    /// Digest of the canonical route state path.
    pub fn route_path_sha256(&self) -> &str {
        &self.route_path_sha256
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct AuthorityReservationV1 {
    schema_name: String,
    schema_version: u32,
    operation_id: String,
    slot: String,
    signed_payload_sha256: String,
}

/// Sender-domain fence: an advisory lock file under the operation-store
/// root keyed by the `{environment, vm, sender}` digest. Acquired before the
/// route lock; every route in the same authority domain serializes here,
/// and a busy result is typed. The file remains in place, while the kernel
/// lock is released automatically on process exit or crash.
#[derive(Debug)]
struct AuthorityDomainLock {
    _file: File,
}

impl AuthorityDomainLock {
    fn acquire(operations_root: &Path, domain_sha256: &str) -> Result<Self> {
        let metadata = fs::symlink_metadata(operations_root).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::InvalidInput(format!(
                    "operation-store root does not exist: {}",
                    operations_root.display()
                ))
            } else {
                Error::Io(error)
            }
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(Error::InvalidInput(format!(
                "operation-store root must be a real directory: {}",
                operations_root.display()
            )));
        }
        let directory = operations_root.join(AUTHORITY_LOCK_DIR);
        fs::create_dir_all(&directory)?;
        let directory_metadata = fs::symlink_metadata(&directory)?;
        if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
            return Err(Error::InvalidInput(format!(
                "authority lock directory must be a real directory: {}",
                directory.display()
            )));
        }
        set_directory_mode(&directory)?;
        let path = directory.join(format!("{domain_sha256}.lock"));
        let file = open_advisory_lock(&path)?;
        try_lock_exclusive(&file).map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                Error::Conflict(format!("authority domain is busy: {}", path.display()))
            } else {
                Error::Io(error)
            }
        })?;
        Ok(Self { _file: file })
    }
}

/// Both mutation locks held — authority-domain first, then route — with the
/// authoritative logs replayed and the Phase-A binding re-verified under
/// them. Hold this guard through the `submission_pending` checkpoint; after
/// it the route lock may be released for long confirmation waits while the
/// authority-domain reservation stays pending in the journal.
#[derive(Debug)]
pub struct SubmissionGuard {
    store: RouteStore,
    reservation_path: PathBuf,
    operation_id: String,
    _route_lock: RouteLock,
    _authority_lock: AuthorityDomainLock,
    state: RouteStateV1,
}

impl SubmissionGuard {
    /// Authoritative store reopened under both locks.
    pub fn store(&self) -> &RouteStore {
        &self.store
    }

    /// Authoritative state snapshot re-read under both locks.
    pub fn state(&self) -> &RouteStateV1 {
        &self.state
    }

    /// Appends the `submission_pending` checkpoint with the signed
    /// transaction/envelope digest while both locks are held. The guard must
    /// not be dropped before this returns.
    pub fn submission_pending(
        &self,
        operation_id: &str,
        signed_transaction_sha256: &str,
        transaction_hash: &str,
        signed_payload: &str,
    ) -> Result<LogRecordV1<OperationEventV1>> {
        if operation_id != self.operation_id {
            return Err(Error::Conflict(
                "submission checkpoint operation differs from the authority reservation".into(),
            ));
        }
        self.store.append_operation(
            OperationEventV1 {
                operation_id: operation_id.into(),
                state: OperationState::SubmissionPending,
                detail: serde_json::json!({
                    "signed_transaction_sha256": signed_transaction_sha256,
                    "transaction_hash": transaction_hash,
                    "signed_payload": signed_payload,
                }),
            },
            None,
        )
    }
    pub fn reserve_authority(&self, slot: &str, signed_payload_sha256: &str) -> Result<()> {
        let reservation = AuthorityReservationV1 {
            schema_name: "authority_reservation".into(),
            schema_version: SCHEMA_VERSION,
            operation_id: self.operation_id.clone(),
            slot: slot.into(),
            signed_payload_sha256: signed_payload_sha256.into(),
        };
        if self.reservation_path.exists() {
            let existing: AuthorityReservationV1 = read_json(&self.reservation_path)?;
            if existing != reservation {
                return Err(Error::Conflict(
                    "authority domain has a different unresolved reservation".into(),
                ));
            }
            return Ok(());
        }
        write_create_new_json(&self.reservation_path, &reservation)
    }

    pub fn release_authority(&self) -> Result<()> {
        if !self.reservation_path.exists() {
            return Ok(());
        }
        let reservation: AuthorityReservationV1 = read_json(&self.reservation_path)?;
        if reservation.operation_id != self.operation_id {
            return Err(Error::Conflict(
                "authority reservation belongs to a different operation".into(),
            ));
        }
        fs::remove_file(&self.reservation_path)?;
        if let Some(parent) = self.reservation_path.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    }

    pub fn release_authority_if_terminal(&self) -> Result<()> {
        let terminal = self
            .store
            .operation_history(&self.operation_id)?
            .last()
            .is_some_and(|event| {
                matches!(
                    event.state,
                    OperationState::Confirmed | OperationState::Failed
                )
            });
        if terminal {
            self.release_authority()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogRecordV1<T> {
    pub schema_name: String,
    pub schema_version: u32,
    pub log_id: String,
    pub index: u64,
    pub previous_record_sha256: String,
    pub companion_artifact_sha256: Option<String>,
    pub canonical_payload_sha256: String,
    pub payload: T,
    pub record_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperationEventV1 {
    pub operation_id: String,
    pub state: OperationState,
    pub detail: serde_json::Value,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Planned,
    ProposalPrepared,
    ProposalCommitted,
    Signed,
    SubmissionPending,
    Confirmed,
    Failed,
    Ambiguous,
}

#[derive(Debug)]
pub struct RouteStore {
    root: PathBuf,
}

impl RouteStore {
    pub fn create(root: &Path, desired: DesiredRouteV1) -> Result<(Self, RouteStateV1)> {
        validate_new_directory(root)?;
        fs::create_dir(root)?;
        set_directory_mode(root)?;

        let desired_sha256 = canonical_sha256(&desired)?;
        let state = RouteStateV1 {
            schema_name: "route_state".into(),
            schema_version: SCHEMA_VERSION,
            route_id: desired.route_id.clone(),
            desired_sha256,
            identity: desired.identity,
            asset: desired.asset,
            opening_custody: None,
            operations_log: PathBuf::from(OPERATIONS_FILE),
            messages_log: PathBuf::from(MESSAGES_FILE),
            lock_file: PathBuf::from(LOCK_FILE),
            contracts: BTreeMap::from([
                ("stellar_owner".into(), desired.stellar_owner),
                ("stellar_delegate".into(), desired.stellar_delegate),
                ("evm_owner".into(), desired.evm_owner),
                ("evm_delegate".into(), desired.evm_delegate),
            ]),
            requested_config: desired.config,
            effective_config: BTreeMap::default(),
        };
        let store = Self {
            root: root.to_path_buf(),
        };
        write_create_new_json(&store.root.join(STATE_FILE), &state)?;
        create_empty_file(&store.root.join(OPERATIONS_FILE))?;
        create_empty_file(&store.root.join(MESSAGES_FILE))?;
        sync_directory(&store.root)?;
        Ok((store, state))
    }

    pub fn open(root: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(root)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(Error::InvalidInput(format!(
                "state path must be a real directory: {}",
                root.display()
            )));
        }
        let store = Self {
            root: root.to_path_buf(),
        };
        let state: RouteStateV1 = read_json(&store.root.join(STATE_FILE))?;
        if state.schema_name != "route_state" || state.schema_version != SCHEMA_VERSION {
            return Err(Error::InvalidInput("unsupported route state schema".into()));
        }
        validate_relative_file(&state.operations_log)?;
        validate_relative_file(&state.messages_log)?;
        validate_relative_file(&state.lock_file)?;
        validate_opening_custody(state.opening_custody.as_ref())?;
        store.verify_log::<OperationEventV1>(&state.operations_log, "operations")?;
        store.verify_log::<MessageRecordV1>(&state.messages_log, "messages")?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn lock(&self) -> Result<RouteLock> {
        RouteLock::acquire(&self.root)
    }

    pub fn load_state(&self) -> Result<RouteStateV1> {
        let state: RouteStateV1 = read_json(&self.root.join(STATE_FILE))?;
        validate_opening_custody(state.opening_custody.as_ref())?;
        Ok(state)
    }

    pub fn save_state(&self, state: &RouteStateV1) -> Result<()> {
        validate_opening_custody(state.opening_custody.as_ref())?;
        atomic_replace_json(&self.root.join(STATE_FILE), state)
    }

    /// Records the immutable finalized custody baseline exactly once.
    pub fn record_opening_custody(&self, opening: crate::domain::OpeningCustodyV1) -> Result<()> {
        let mut state = self.load_state()?;
        if state.opening_custody.is_some() {
            return Err(Error::Conflict(
                "opening custody baseline is already recorded".into(),
            ));
        }
        state.opening_custody = Some(opening);
        self.save_state(&state)
    }
    pub fn append_operation(
        &self,
        payload: OperationEventV1,
        companion_artifact_sha256: Option<String>,
    ) -> Result<LogRecordV1<OperationEventV1>> {
        let previous = self
            .verify_log::<OperationEventV1>(Path::new(OPERATIONS_FILE), "operations")?
            .into_iter()
            .rev()
            .find(|record| record.payload.operation_id == payload.operation_id)
            .map(|record| record.payload.state);
        if !valid_operation_transition(previous, payload.state) {
            return Err(Error::Conflict(format!(
                "invalid operation transition for {}: {previous:?} -> {:?}",
                payload.operation_id, payload.state
            )));
        }
        self.append_log(
            OPERATIONS_FILE,
            "operations",
            payload,
            companion_artifact_sha256,
        )
    }

    pub fn operation_history(&self, operation_id: &str) -> Result<Vec<OperationEventV1>> {
        Ok(self
            .verify_log::<OperationEventV1>(Path::new(OPERATIONS_FILE), "operations")?
            .into_iter()
            .filter_map(|record| {
                (record.payload.operation_id == operation_id).then_some(record.payload)
            })
            .collect())
    }

    /// Appends a brand-new packet record. Identity is the append-only key
    /// `(source_eid, sender, nonce, guid)`; duplicates are conflicts.
    pub fn append_message(&self, mut record: MessageRecordV1) -> Result<()> {
        self.validate_new_message(&record)?;
        let existing = self.load_messages()?;
        if existing.iter().any(|other| {
            other.guid == record.guid
                || (other.source_eid, &other.sender, &other.nonce)
                    == (record.source_eid, &record.sender, &record.nonce)
        }) {
            return Err(Error::Conflict(format!(
                "message nonce or GUID already recorded: guid {}",
                record.guid
            )));
        }
        record.schema_name = "message_record".into();
        record.schema_version = SCHEMA_VERSION;
        self.append_log(MESSAGES_FILE, "messages", record, None)?;
        Ok(())
    }

    /// Returns whether authenticated packet evidence for a source transaction
    /// is already present in the append-only message ledger. A send operation
    /// must not become terminal or release its authority reservation until
    /// this is true.
    pub fn has_message_for_source_transaction(&self, transaction_hash: &str) -> Result<bool> {
        Ok(self.load_messages()?.iter().any(|record| {
            record
                .source_transaction
                .eq_ignore_ascii_case(transaction_hash)
        }))
    }

    /// Shared acceptance checks for a brand-new packet record. Identity
    /// uniqueness is checked separately against the target ledger context.
    fn validate_new_message(&self, record: &MessageRecordV1) -> Result<()> {
        if record.status_events.is_empty() {
            return Err(Error::InvalidInput(
                "message record requires an initial status event".into(),
            ));
        }
        if record
            .status_events
            .iter()
            .any(|event| event.stage == MessageStageV1::Reobserved)
        {
            return Err(Error::InvalidInput(
                "initial message status must be an observed stage, not reobserved".into(),
            ));
        }
        let initial = record.status_events[0].stage;
        let direction_matches = match record.direction {
            Direction::StellarToEvm => matches!(
                initial,
                MessageStageV1::ForwardSourceAccepted
                    | MessageStageV1::ForwardLocked
                    | MessageStageV1::ForwardVerified
                    | MessageStageV1::ForwardCommitted
                    | MessageStageV1::ForwardMinted
            ),
            Direction::EvmToStellar => matches!(
                initial,
                MessageStageV1::ReverseSourceAccepted
                    | MessageStageV1::ReverseBurned
                    | MessageStageV1::ReverseVerified
                    | MessageStageV1::ReverseCommitted
                    | MessageStageV1::ReverseUnlocked
            ),
        };
        if !direction_matches {
            return Err(Error::InvalidInput(
                "initial message status does not match packet direction".into(),
            ));
        }
        if record.packet_sha256.trim().is_empty()
            || record.packet_header.trim().is_empty()
            || record.message.trim().is_empty()
            || record.payload_keccak256.trim().is_empty()
            || record.origin.trim().is_empty()
            || record.receiver.trim().is_empty()
            || record.current_receive_library.trim().is_empty()
            || record.send_library.trim().is_empty()
            || record.uln_snapshot_sha256.trim().is_empty()
            || record.dvn_snapshot_sha256.trim().is_empty()
            || record.executor_snapshot_sha256.trim().is_empty()
            || record.config_snapshot_sha256.trim().is_empty()
            || record.source_height.trim().is_empty()
            || record.source_event_coordinate.trim().is_empty()
            || record.source_transaction.trim().is_empty()
        {
            return Err(Error::InvalidInput(
                "message record requires complete packet, chain-coordinate, and config evidence"
                    .into(),
            ));
        }
        crate::layerzero::qualify_message_for_route(&self.load_state()?, record)?;
        Ok(())
    }

    /// Appends a batch of brand-new packet records as one durable write.
    /// Every record passes the same acceptance checks as [`append_message`]
    /// and identity uniqueness is enforced across the existing ledger and
    /// within the batch before any byte is written, so a rejected record
    /// cannot strand an earlier one in the log.
    pub fn append_messages_batch(&self, mut records: Vec<MessageRecordV1>) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let existing = self.verify_log::<MessageRecordV1>(Path::new(MESSAGES_FILE), "messages")?;
        let mut seen_nonces: BTreeSet<(u32, String, String)> = existing
            .iter()
            .map(|record| {
                (
                    record.payload.source_eid,
                    record.payload.sender.clone(),
                    record.payload.nonce.clone(),
                )
            })
            .collect();
        let mut seen_guids: BTreeSet<String> = existing
            .iter()
            .map(|record| record.payload.guid.clone())
            .collect();
        for record in &records {
            self.validate_new_message(record)?;
            if !seen_nonces.insert((
                record.source_eid,
                record.sender.clone(),
                record.nonce.clone(),
            )) || !seen_guids.insert(record.guid.clone())
            {
                return Err(Error::Conflict(format!(
                    "message nonce or GUID already recorded: guid {}",
                    record.guid
                )));
            }
        }
        for record in &mut records {
            record.schema_name = "message_record".into();
            record.schema_version = SCHEMA_VERSION;
        }
        self.append_log_records(
            MESSAGES_FILE,
            "messages",
            records.into_iter().map(|record| (record, None)).collect(),
        )?;
        Ok(())
    }

    /// Appends a status event to an existing packet. The ledger is
    /// append-only: history is never rewritten, the event lands as a new
    /// chained record carrying the updated snapshot. Immutable fields
    /// (packet/payload/config digests, amounts, coordinates) must not change.
    pub fn append_message_event(
        &self,
        identity: &(u32, String, String, String),
        event: MessageStatusEventV1,
    ) -> Result<()> {
        let mut latest = self
            .find_message(identity)?
            .ok_or_else(|| Error::Conflict("message identity not recorded".into()))?;
        let last = latest
            .status_events
            .iter()
            .rev()
            .find(|event| event.stage != MessageStageV1::Reobserved)
            .ok_or_else(|| Error::Custody("recorded message lacks a custody status event".into()))?
            .stage;
        if !valid_message_transition(latest.direction, last, event.stage) {
            return Err(Error::InvalidInput(format!(
                "non-monotonic message status {:?} after {:?}",
                event.stage, last
            )));
        }
        latest.status_events.push(event);

        self.append_log(MESSAGES_FILE, "messages", latest, None)?;
        Ok(())
    }
    pub fn append_message_recovery_event(
        &self,
        identity: &(u32, String, String, String),
        transaction: String,
        event: MessageStatusEventV1,
    ) -> Result<()> {
        let mut latest = self
            .find_message(identity)?
            .ok_or_else(|| Error::Conflict("message identity not recorded".into()))?;
        let last = latest
            .status_events
            .iter()
            .rev()
            .find(|event| event.stage != MessageStageV1::Reobserved)
            .ok_or_else(|| Error::Custody("recorded message lacks a custody status event".into()))?
            .stage;
        if !valid_message_transition(latest.direction, last, event.stage) {
            return Err(Error::InvalidInput(format!(
                "non-monotonic recovery status {:?} after {:?}",
                event.stage, last
            )));
        }
        if latest
            .recovery_transactions
            .iter()
            .any(|recorded| recorded.eq_ignore_ascii_case(&transaction))
        {
            return Err(Error::Conflict(
                "recovery transaction is already recorded".into(),
            ));
        }
        latest.recovery_transactions.push(transaction);
        latest.status_events.push(event);
        self.append_log(MESSAGES_FILE, "messages", latest, None)?;
        Ok(())
    }

    pub fn append_message_destination_event(
        &self,
        identity: &(u32, String, String, String),
        transaction: String,
        event: MessageStatusEventV1,
    ) -> Result<()> {
        let mut latest = self
            .find_message(identity)?
            .ok_or_else(|| Error::Conflict("message identity not recorded".into()))?;
        let last = latest
            .status_events
            .iter()
            .rev()
            .find(|event| event.stage != MessageStageV1::Reobserved)
            .ok_or_else(|| Error::Custody("recorded message lacks a custody status event".into()))?
            .stage;
        if !valid_message_transition(latest.direction, last, event.stage) {
            return Err(Error::InvalidInput(format!(
                "non-monotonic destination status {:?} after {:?}",
                event.stage, last
            )));
        }
        if latest.destination_transaction.is_some() {
            return Err(Error::Conflict(
                "destination transaction is already recorded".into(),
            ));
        }
        latest.destination_transaction = Some(transaction);
        latest.status_events.push(event);
        self.append_log(MESSAGES_FILE, "messages", latest, None)?;
        Ok(())
    }

    /// Loads every packet record folded to its latest snapshot, verifying the
    /// hash chain and identity uniqueness across history.
    pub fn load_messages(&self) -> Result<Vec<MessageRecordV1>> {
        let records = self.verify_log::<MessageRecordV1>(Path::new(MESSAGES_FILE), "messages")?;
        let mut folded: Vec<MessageRecordV1> = Vec::new();
        for record in records {
            let payload = record.payload;
            match folded
                .iter()
                .position(|existing| existing.identity() == payload.identity())
            {
                Some(index) => {
                    let existing = &folded[index];
                    if existing.packet_sha256 != payload.packet_sha256
                        || existing.packet_header != payload.packet_header
                        || existing.message != payload.message
                        || existing.payload_keccak256 != payload.payload_keccak256
                        || existing.origin != payload.origin
                        || existing.receiver != payload.receiver
                        || existing.current_receive_library != payload.current_receive_library
                        || existing.old_receive_library != payload.old_receive_library
                        || existing.receive_grace_until != payload.receive_grace_until
                        || existing.send_library != payload.send_library
                        || existing.uln_snapshot_sha256 != payload.uln_snapshot_sha256
                        || existing.dvn_snapshot_sha256 != payload.dvn_snapshot_sha256
                        || existing.executor_snapshot_sha256 != payload.executor_snapshot_sha256
                        || existing.config_snapshot_sha256 != payload.config_snapshot_sha256
                        || existing.source_height != payload.source_height
                        || existing.source_event_coordinate != payload.source_event_coordinate
                        || existing.amount_raw != payload.amount_raw
                        || existing.source_transaction != payload.source_transaction
                        || existing.debited_raw != payload.debited_raw
                        || existing.net_locked_raw != payload.net_locked_raw
                        || existing.minted_raw != payload.minted_raw
                        || existing.burned_raw != payload.burned_raw
                        || existing.unlocked_raw != payload.unlocked_raw
                        || existing.external_fee_raw != payload.external_fee_raw
                        || existing.dust_raw != payload.dust_raw
                    {
                        return Err(Error::Custody(format!(
                            "immutable fields changed for guid {}",
                            payload.guid
                        )));
                    }
                    if payload.status_events.len() != existing.status_events.len() + 1 {
                        return Err(Error::Custody(format!(
                            "status history is not append-only for guid {}",
                            payload.guid
                        )));
                    }
                    folded[index] = payload;
                }
                None => folded.push(payload),
            }
        }
        Ok(folded)
    }

    fn find_message(
        &self,
        identity: &(u32, String, String, String),
    ) -> Result<Option<MessageRecordV1>> {
        Ok(self
            .load_messages()?
            .into_iter()
            .find(|record| &record.identity() == identity))
    }

    pub fn write_proposal<T: Serialize>(
        &self,
        relative_path: &Path,
        operation_id: &str,
        proposal: &T,
    ) -> Result<ArtifactRefV1> {
        validate_relative_file(relative_path)?;
        let final_path = self.root.join(relative_path);
        ensure_parent_under(&self.root, &final_path)?;
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
            set_directory_mode(parent)?;
        }
        if final_path.exists() {
            return Err(Error::Conflict(format!(
                "artifact path already exists: {}",
                final_path.display()
            )));
        }

        let bytes = serde_json_canonicalizer::to_vec(proposal)?;
        let sha256 = hex::encode(Sha256::digest(&bytes));
        let temporary = final_path.with_extension("tmp");
        write_create_new_bytes(&temporary, &bytes)?;
        self.append_operation(
            OperationEventV1 {
                operation_id: operation_id.into(),
                state: OperationState::ProposalPrepared,
                detail: serde_json::json!({"path": relative_path, "sha256": sha256}),
            },
            Some(sha256.clone()),
        )?;
        fs::rename(&temporary, &final_path)?;
        if let Some(parent) = final_path.parent() {
            sync_directory(parent)?;
        }
        self.append_operation(
            OperationEventV1 {
                operation_id: operation_id.into(),
                state: OperationState::ProposalCommitted,
                detail: serde_json::json!({"path": relative_path, "sha256": sha256}),
            },
            Some(sha256.clone()),
        )?;
        Ok(ArtifactRefV1 {
            kind: "proposal".into(),
            path: relative_path.to_path_buf(),
            sha256,
            schema_version: SCHEMA_VERSION,
            authoritative: true,
        })
    }

    pub fn verify_log<T: DeserializeOwned + Serialize>(
        &self,
        relative_path: &Path,
        log_kind: &str,
    ) -> Result<Vec<LogRecordV1<T>>> {
        validate_relative_file(relative_path)?;
        let path = self.root.join(relative_path);
        let file = open_regular_read(&path)?;
        let mut records = Vec::new();
        let mut previous = genesis_hash(log_kind, &self.load_state()?.route_id);
        for (line_index, line) in BufReader::new(file).lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let record: LogRecordV1<T> = serde_json::from_str(&line).map_err(|error| {
                Error::Custody(format!(
                    "invalid {log_kind} log record {}: {error}",
                    line_index + 1
                ))
            })?;
            if record.index != records.len() as u64 || record.previous_record_sha256 != previous {
                return Err(Error::Custody(format!(
                    "{log_kind} log chain mismatch at record {}",
                    record.index
                )));
            }
            let expected = record_hash(&record)?;
            if expected != record.record_sha256 {
                return Err(Error::Custody(format!(
                    "{log_kind} log digest mismatch at record {}",
                    record.index
                )));
            }
            previous.clone_from(&record.record_sha256);
            records.push(record);
        }
        Ok(records)
    }

    fn append_log<T: Clone + DeserializeOwned + Serialize>(
        &self,
        file_name: &str,
        log_kind: &str,
        payload: T,
        companion_artifact_sha256: Option<String>,
    ) -> Result<LogRecordV1<T>> {
        let mut records = self.append_log_records(
            file_name,
            log_kind,
            vec![(payload, companion_artifact_sha256)],
        )?;
        Ok(records.remove(0))
    }

    /// Builds the chained records for `payloads` and durably appends them in
    /// one open/write/fsync pass. Chain indices, previous-record digests, and
    /// canonical payload digests are computed exactly as for a sequence of
    /// single [`Self::append_log`] calls, so the written bytes are identical
    /// to appending one record at a time. All chain construction happens
    /// before the file is opened, so callers that pre-validate the payloads
    /// get an all-or-nothing append.
    fn append_log_records<T: Clone + DeserializeOwned + Serialize>(
        &self,
        file_name: &str,
        log_kind: &str,
        payloads: Vec<(T, Option<String>)>,
    ) -> Result<Vec<LogRecordV1<T>>> {
        let existing = self.verify_log::<T>(Path::new(file_name), log_kind)?;
        let state = self.load_state()?;
        let mut records = Vec::with_capacity(payloads.len());
        let mut previous = existing.last().map_or_else(
            || genesis_hash(log_kind, &state.route_id),
            |record| record.record_sha256.clone(),
        );
        let mut index = existing.len() as u64;
        for (payload, companion_artifact_sha256) in payloads {
            let canonical_payload_sha256 = canonical_sha256(&payload)?;
            let mut record = LogRecordV1 {
                schema_name: format!("{log_kind}_log_record"),
                schema_version: SCHEMA_VERSION,
                log_id: format!("{}:{log_kind}", state.route_id),
                index,
                previous_record_sha256: previous,
                companion_artifact_sha256,
                canonical_payload_sha256,
                payload,
                record_sha256: String::new(),
            };
            record.record_sha256 = record_hash(&record)?;
            previous = record.record_sha256.clone();
            index += 1;
            records.push(record);
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(self.root.join(file_name))?;
        for record in &records {
            serde_json::to_writer(&mut file, record)?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        Ok(records)
    }

    /// Phase A: non-authoritative, read-only derivation of the authority
    /// binding from the current state file: `{environment, vm, sender}`
    /// plus the canonical route path, with digests. Verifies no log,
    /// reserves no nonce, and trusts no mutable decision;
    /// `acquire_mutation` re-derives the same binding under both locks and
    /// rejects it if anything changed in between.
    pub fn derive_phase_a_binding(&self, vm: Vm, sender: &str) -> Result<PhaseABinding> {
        let state = self.load_state()?;
        PhaseABinding::derive(&self.root, state.identity.environment, vm, sender)
    }

    /// Fixed mutation lock order: authority-domain lock under
    /// `operations_root` first, then the route lock. Under both locks the
    /// authoritative logs are replayed, state is re-read, and the Phase-A
    /// binding must be unchanged; a stale binding releases every lock and
    /// errors so the caller restarts Phase A. The returned guard must be
    /// held through the `submission_pending` checkpoint.
    pub fn acquire_mutation(
        &self,
        binding: &PhaseABinding,
        operations_root: &Path,
        operation_id: &str,
    ) -> Result<SubmissionGuard> {
        let authority_lock =
            AuthorityDomainLock::acquire(operations_root, binding.domain_sha256())?;
        let reservation_path = operations_root
            .join(AUTHORITY_LOCK_DIR)
            .join(format!("{}.reservation.json", binding.domain_sha256()));
        if reservation_path.exists() {
            let reservation: AuthorityReservationV1 = read_json(&reservation_path)?;
            if reservation.operation_id != operation_id {
                return Err(Error::Conflict(
                    "authority domain has an unresolved submission reservation".into(),
                ));
            }
        }
        let route_lock = RouteLock::acquire(binding.route_canonical_path())?;
        let store = RouteStore::open(binding.route_canonical_path())?;
        let state = store.load_state()?;
        let rederived = PhaseABinding::derive(
            binding.route_canonical_path(),
            state.identity.environment,
            binding.vm(),
            binding.sender(),
        )?;
        if &rederived != binding {
            return Err(Error::Conflict(format!(
                "phase-a binding is stale: state changed since derivation (domain {})",
                binding.domain_sha256()
            )));
        }
        Ok(SubmissionGuard {
            store,
            reservation_path,
            operation_id: operation_id.into(),
            _route_lock: route_lock,
            _authority_lock: authority_lock,
            state,
        })
    }

    /// Restart recovery for a `proposal_prepared` checkpoint that has no
    /// matching `proposal_committed`. The caller must hold the route lock.
    /// Replays the authoritative operations log to find pending prepared
    /// records, then for each: a final artifact whose SHA-256 matches
    /// completes the commit record; with no valid final, a matching
    /// temporary is atomically renamed over the final path and the directory
    /// fsynced; a mismatched or non-regular occupant of the final path (or a
    /// missing artifact entirely) fails closed with `proposal_write_failed`
    pub fn recover_pending_proposal(&self) -> Result<()> {
        let records =
            self.verify_log::<OperationEventV1>(Path::new(OPERATIONS_FILE), "operations")?;
        for (index, record) in records.iter().enumerate() {
            if record.payload.state != OperationState::ProposalPrepared {
                continue;
            }
            let Some((relative_path, expected_sha256)) = proposal_write_target(&record.payload)
            else {
                continue;
            };
            let already_committed = records[index + 1..].iter().any(|later| {
                later.payload.state == OperationState::ProposalCommitted
                    && later.payload.operation_id == record.payload.operation_id
                    && proposal_write_target(&later.payload)
                        .is_some_and(|(path, _)| path == relative_path)
            });
            if already_committed {
                continue;
            }
            self.complete_proposal_write(
                relative_path,
                &record.payload.operation_id,
                expected_sha256,
            )?;
        }
        Ok(())
    }

    fn complete_proposal_write(
        &self,
        relative_path: &Path,
        operation_id: &str,
        expected_sha256: &str,
    ) -> Result<()> {
        let final_path = self.root.join(relative_path);
        let temporary = final_path.with_extension("tmp");
        match regular_file_state(&final_path)? {
            FilePresence::Regular(sha256) if sha256 == expected_sha256 => {
                remove_orphan_temporary(&temporary)?;
                self.append_proposal_committed(relative_path, operation_id, expected_sha256)?;
                Ok(())
            }
            FilePresence::Regular(_) => Err(Error::Conflict(format!(
                "proposal_write_failed: final artifact {} does not match recorded sha256 {expected_sha256}",
                final_path.display()
            ))),
            FilePresence::NonRegular => Err(Error::Conflict(format!(
                "proposal_write_failed: final path occupied by a non-regular file: {}",
                final_path.display()
            ))),
            FilePresence::Absent => match regular_file_state(&temporary)? {
                FilePresence::Regular(sha256) if sha256 == expected_sha256 => {
                    fs::rename(&temporary, &final_path)?;
                    if let Some(parent) = final_path.parent() {
                        sync_directory(parent)?;
                    }
                    self.append_proposal_committed(relative_path, operation_id, expected_sha256)?;
                    Ok(())
                }
                _ => Err(Error::Custody(format!(
                    "proposal_write_failed: no artifact matches recorded sha256 {expected_sha256} \
                     (final {} and temporary {} absent or mismatched)",
                    final_path.display(),
                    temporary.display()
                ))),
            },
        }
    }

    fn append_proposal_committed(
        &self,
        relative_path: &Path,
        operation_id: &str,
        expected_sha256: &str,
    ) -> Result<()> {
        self.append_operation(
            OperationEventV1 {
                operation_id: operation_id.into(),
                state: OperationState::ProposalCommitted,
                detail: serde_json::json!({"path": relative_path, "sha256": expected_sha256}),
            },
            Some(expected_sha256.into()),
        )?;
        Ok(())
    }
}

fn route_path_sha256_digest(path: &Path) -> String {
    hex::encode(Sha256::digest(path.as_os_str().as_encoded_bytes()))
}

fn proposal_write_target(event: &OperationEventV1) -> Option<(&Path, &str)> {
    let path = event.detail.get("path")?.as_str()?;
    let digest = event.detail.get("sha256")?.as_str()?;
    Some((Path::new(path), digest))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FilePresence {
    Absent,
    Regular(String),
    NonRegular,
}

fn regular_file_state(path: &Path) -> Result<FilePresence> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            Ok(FilePresence::NonRegular)
        }
        Ok(_) => {
            let mut reader = File::open(path)?;
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 8192];
            loop {
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
            Ok(FilePresence::Regular(hex::encode(hasher.finalize())))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FilePresence::Absent),
        Err(error) => Err(Error::Io(error)),
    }
}

fn remove_orphan_temporary(temporary: &Path) -> Result<()> {
    match fs::remove_file(temporary) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}

fn canonical_authority_sender(vm: Vm, sender: &str) -> Result<String> {
    match vm {
        Vm::Evm => crate::evm::parse_address(sender).map(crate::evm::canonical_address),
        Vm::Stellar => Ok(sender.to_owned()),
    }
}

fn open_advisory_lock(path: &Path) -> Result<File> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(Error::InvalidInput(format!(
                "lock path must be a real file: {}",
                path.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::Io(error)),
    }

    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    let path_metadata = fs::symlink_metadata(path)?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(Error::InvalidInput(format!(
            "lock path must be a real file: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let file_metadata = file.metadata()?;
        if path_metadata.dev() != file_metadata.dev()
            || path_metadata.ino() != file_metadata.ino()
            || file_metadata.nlink() != 1
        {
            return Err(Error::InvalidInput(format!(
                "lock path must not be replaced or hard-linked: {}",
                path.display()
            )));
        }
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> std::io::Result<()> {
    const LOCK_EXCLUSIVE: std::os::raw::c_int = 2;
    const LOCK_NONBLOCKING: std::os::raw::c_int = 4;
    unsafe extern "C" {
        fn flock(fd: std::os::raw::c_int, operation: std::os::raw::c_int) -> std::os::raw::c_int;
    }
    // SAFETY: `file` owns a valid descriptor for the duration of this call;
    // `flock` neither retains the pointer nor accesses Rust memory.
    if unsafe { flock(file.as_raw_fd(), LOCK_EXCLUSIVE | LOCK_NONBLOCKING) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn try_lock_exclusive(_file: &File) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "advisory operation-store locks require a Unix platform",
    ))
}

fn record_hash<T: Serialize>(record: &LogRecordV1<T>) -> Result<String> {
    #[derive(Serialize)]
    struct HashInput<'a, T> {
        schema_name: &'a str,
        schema_version: u32,
        log_id: &'a str,
        index: u64,
        previous_record_sha256: &'a str,
        companion_artifact_sha256: &'a Option<String>,
        canonical_payload_sha256: &'a str,
        payload: &'a T,
    }
    canonical_sha256(&HashInput {
        schema_name: &record.schema_name,
        schema_version: record.schema_version,
        log_id: &record.log_id,
        index: record.index,
        previous_record_sha256: &record.previous_record_sha256,
        companion_artifact_sha256: &record.companion_artifact_sha256,
        canonical_payload_sha256: &record.canonical_payload_sha256,
        payload: &record.payload,
    })
}

fn genesis_hash(log_kind: &str, route_id: &str) -> String {
    hex::encode(Sha256::digest(format!(
        "templar-oft-bridge-log-v1{route_id}{log_kind}"
    )))
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = open_regular_read(path)?;
    Ok(serde_json::from_reader(file)?)
}

pub fn write_create_new_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json_canonicalizer::to_vec(value)?;
    write_create_new_bytes(path, &bytes)
}

fn atomic_replace_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    atomic_replace_bytes(path, &serde_json_canonicalizer::to_vec(value)?)
}

/// Atomically replaces a managed file with `bytes`: unique temp write+fsync,
/// rename over the target, directory fsync. Any failure before the rename
/// leaves the prior file intact.
fn atomic_replace_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidInput("state path has no parent".into()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::InvalidInput("state path has an invalid file name".into()))?;
    let temporary = parent.join(format!(".{file_name}.tmp"));
    if temporary.exists() {
        return Err(Error::Conflict(format!(
            "temporary state path exists: {}",
            temporary.display()
        )));
    }
    write_create_new_bytes(&temporary, bytes)?;
    fs::rename(&temporary, path)?;
    sync_directory(parent)
}

fn write_create_new_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn create_empty_file(path: &Path) -> Result<()> {
    write_create_new_bytes(path, b"")
}

fn open_regular_read(path: &Path) -> Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(Error::InvalidInput(format!(
            "path must be a real file: {}",
            path.display()
        )));
    }
    Ok(File::open(path)?)
}

fn validate_new_directory(path: &Path) -> Result<()> {
    if path.exists() {
        return Err(Error::Conflict(format!(
            "state directory already exists: {}",
            path.display()
        )));
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(Error::InvalidInput("state path must not contain ..".into()));
    }
    Ok(())
}

fn validate_relative_file(path: &Path) -> Result<()> {
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(Error::InvalidInput(format!(
            "managed path must remain relative: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_parent_under(root: &Path, path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::InvalidInput("artifact path has no parent".into()))?;
    if !parent.starts_with(root) {
        return Err(Error::InvalidInput(
            "artifact escapes route directory".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn set_directory_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_directory_mode(_path: &Path) -> Result<()> {
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

pub fn validate_opening_custody(opening: Option<&crate::domain::OpeningCustodyV1>) -> Result<()> {
    let Some(opening) = opening else {
        return Ok(());
    };
    if opening.schema_name != "opening_custody" || opening.schema_version != SCHEMA_VERSION {
        return Err(Error::InvalidInput(
            "unsupported opening custody schema".into(),
        ));
    }
    if opening.stellar_ledger_hash.trim().is_empty()
        || opening.evm_block_hash.trim().is_empty()
        || opening.artifact_lock_sha256.trim().is_empty()
        || opening.effective_config_sha256.trim().is_empty()
    {
        return Err(Error::Custody(
            "opening custody requires finalized chain and configuration digests".into(),
        ));
    }
    let evidence = opening
        .history_evidence_sha256
        .as_deref()
        .is_some_and(|digest| !digest.trim().is_empty());
    if opening.zero_packet_history_proven == evidence {
        return Err(Error::Custody(
            "opening custody requires exactly one of zero-history proof or imported history".into(),
        ));
    }
    Ok(())
}

fn valid_operation_transition(previous: Option<OperationState>, next: OperationState) -> bool {
    matches!(
        (previous, next),
        (
            None,
            OperationState::Planned | OperationState::ProposalPrepared
        ) | (
            Some(OperationState::Planned),
            OperationState::ProposalPrepared | OperationState::Signed | OperationState::Failed
        ) | (
            Some(OperationState::ProposalPrepared),
            OperationState::ProposalCommitted
        ) | (
            Some(OperationState::ProposalCommitted),
            OperationState::Signed | OperationState::Confirmed | OperationState::Failed
        ) | (
            Some(OperationState::Signed),
            OperationState::SubmissionPending
        ) | (
            Some(OperationState::SubmissionPending | OperationState::Ambiguous),
            OperationState::Confirmed | OperationState::Failed | OperationState::Ambiguous
        )
    )
}
