use logcore::session::Session;
use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasher, Hash, Hasher};
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MAX_ACTIVE_PROBLEM_SNAPSHOTS: usize = 8;
const MAX_RETIRED_PROBLEM_CAPABILITIES: usize = 64;
const PROBLEM_SNAPSHOT_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProblemAnalysisIdentity {
    pub session_generation: u64,
    pub analysis_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProblemPageQuery {
    Groups { kind: Option<u8>, sort: u8 },
    Occurrences { group_id: u32 },
}

impl ProblemPageQuery {
    fn signature(self) -> u64 {
        match self {
            Self::Groups { kind, sort } => {
                0x4752_4f55_5000_0000 | u64::from(kind.unwrap_or(u8::MAX)) << 8 | u64::from(sort)
            }
            Self::Occurrences { group_id } => 0x4f43_4355_0000_0000 | u64::from(group_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProblemCursorError {
    Invalid,
    Tampered,
    Replayed,
    InUse,
    QueryMismatch,
    AnalysisMismatch,
    Released,
    Expired,
    Evicted,
    Capacity,
}

impl ProblemCursorError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Invalid => "problem-cursor-invalid",
            Self::Tampered => "problem-cursor-tampered",
            Self::Replayed => "problem-cursor-replayed",
            Self::InUse => "problem-cursor-in-use",
            Self::QueryMismatch => "problem-cursor-query-mismatch",
            Self::AnalysisMismatch => "problem-cursor-analysis-mismatch",
            Self::Released => "problem-snapshot-handle-released",
            Self::Expired => "snapshot-expired",
            Self::Evicted => "snapshot-evicted",
            Self::Capacity => "problem-cursor-capacity",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProblemSnapshotLease {
    pub snapshot_id: u64,
    pub position: usize,
    pub query: ProblemPageQuery,
    handle_id: u64,
    operation_id: u64,
    cursor_id: Option<u64>,
    pub snapshot_handle: String,
}

impl ProblemSnapshotLease {
    pub const fn is_initial_page(&self) -> bool {
        self.cursor_id.is_none()
    }
}

#[derive(Debug, Clone)]
struct ProblemSnapshotRecord {
    snapshot_id: u64,
    analysis: ProblemAnalysisIdentity,
    query: ProblemPageQuery,
    last_used: u64,
    expires_at: Instant,
    in_flight: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProblemCursorState {
    Fresh,
    Reserved(u64),
}

#[derive(Debug, Clone)]
struct ProblemCursorRecord {
    handle_id: u64,
    snapshot_id: u64,
    analysis: ProblemAnalysisIdentity,
    query: ProblemPageQuery,
    position: usize,
    state: ProblemCursorState,
}

#[derive(Debug, Clone, Copy)]
enum RetiredSnapshotReason {
    Released,
    Expired,
    Evicted,
}

impl RetiredSnapshotReason {
    const fn error(self) -> ProblemCursorError {
        match self {
            Self::Released => ProblemCursorError::Released,
            Self::Expired => ProblemCursorError::Expired,
            Self::Evicted => ProblemCursorError::Evicted,
        }
    }
}

#[derive(Debug, Clone)]
struct RetiredSnapshotRecord {
    snapshot: ProblemSnapshotRecord,
    reason: RetiredSnapshotReason,
}

#[derive(Debug, Clone, Copy)]
enum RetiredCapability {
    Snapshot(u64),
    Cursor(u64),
}

#[derive(Default)]
struct ProblemCursorRegistryInner {
    next_id: u64,
    next_access: u64,
    snapshots: HashMap<u64, ProblemSnapshotRecord>,
    retired_snapshots: HashMap<u64, RetiredSnapshotRecord>,
    cursors: HashMap<u64, ProblemCursorRecord>,
    retired_cursors: HashMap<u64, ProblemCursorRecord>,
    retired_order: VecDeque<RetiredCapability>,
    pending_core_releases: Vec<u64>,
}

/// Server-owned capability registry for Problems pagination.
///
/// Tokens only reveal a registry nonce and a keyed authenticator; snapshot ids,
/// positions and query signatures stay server-side. The two independently
/// seeded `RandomState`s make the authenticator process-local and 128-bit.
pub struct ProblemCursorRegistry {
    mac_left: RandomState,
    mac_right: RandomState,
    ttl: Duration,
    inner: Mutex<ProblemCursorRegistryInner>,
}

impl ProblemCursorRegistry {
    pub fn new() -> Self {
        Self::with_ttl(PROBLEM_SNAPSHOT_TTL)
    }

    fn with_ttl(ttl: Duration) -> Self {
        Self {
            mac_left: RandomState::new(),
            mac_right: RandomState::new(),
            ttl,
            inner: Mutex::new(ProblemCursorRegistryInner::default()),
        }
    }

    pub fn register_snapshot(
        &self,
        snapshot_id: u64,
        analysis: ProblemAnalysisIdentity,
        query: ProblemPageQuery,
    ) -> Result<ProblemSnapshotLease, ProblemCursorError> {
        let mut inner = self.lock_inner();
        self.sweep_expired_locked(&mut inner, Instant::now());
        if inner.snapshots.len() >= MAX_ACTIVE_PROBLEM_SNAPSHOTS {
            let lru = inner
                .snapshots
                .iter()
                .filter(|(_, record)| record.in_flight.is_none())
                .min_by_key(|(_, record)| record.last_used)
                .map(|(handle_id, _)| *handle_id)
                .ok_or(ProblemCursorError::Capacity)?;
            retire_snapshot_locked(&mut inner, lru, RetiredSnapshotReason::Evicted, true);
        }
        let handle_id = next_registry_id(&mut inner)?;
        let operation_id = next_registry_id(&mut inner)?;
        let last_used = next_access_id(&mut inner)?;
        let record = ProblemSnapshotRecord {
            snapshot_id,
            analysis,
            query,
            last_used,
            expires_at: Instant::now() + self.ttl,
            in_flight: Some(operation_id),
        };
        let snapshot_handle = self.snapshot_handle(handle_id, &record);
        inner.snapshots.insert(handle_id, record);
        Ok(ProblemSnapshotLease {
            snapshot_id,
            position: 0,
            query,
            handle_id,
            operation_id,
            cursor_id: None,
            snapshot_handle,
        })
    }

    pub fn reserve_cursor(
        &self,
        cursor: &str,
        analysis: ProblemAnalysisIdentity,
        expected_query: Option<ProblemPageQuery>,
    ) -> Result<ProblemSnapshotLease, ProblemCursorError> {
        let (cursor_id, supplied_mac) = parse_capability(cursor, "pc1")?;
        let mut inner = self.lock_inner();
        self.sweep_expired_locked(&mut inner, Instant::now());
        let Some(record) = inner.cursors.get(&cursor_id) else {
            if let Some(retired) = inner.retired_cursors.get(&cursor_id) {
                let expected_mac = self.cursor_mac(cursor_id, retired);
                return if constant_time_eq(&supplied_mac, &expected_mac) {
                    Err(ProblemCursorError::Replayed)
                } else {
                    Err(ProblemCursorError::Tampered)
                };
            }
            return Err(ProblemCursorError::Invalid);
        };
        let expected_mac = self.cursor_mac(cursor_id, record);
        if !constant_time_eq(&supplied_mac, &expected_mac) {
            return Err(ProblemCursorError::Tampered);
        }
        if matches!(record.state, ProblemCursorState::Reserved(_)) {
            return Err(ProblemCursorError::InUse);
        }
        if record.analysis != analysis {
            return Err(ProblemCursorError::AnalysisMismatch);
        }
        if expected_query.is_some_and(|query| query != record.query) {
            return Err(ProblemCursorError::QueryMismatch);
        }
        let handle_id = record.handle_id;
        let snapshot_id = record.snapshot_id;
        let query = record.query;
        let position = record.position;
        let snapshot = inner
            .snapshots
            .get(&handle_id)
            .ok_or(ProblemCursorError::Invalid)?;
        if snapshot.snapshot_id != snapshot_id
            || snapshot.analysis != analysis
            || snapshot.query != query
        {
            return Err(ProblemCursorError::Tampered);
        }
        if snapshot.in_flight.is_some() {
            return Err(ProblemCursorError::InUse);
        }
        let operation_id = next_registry_id(&mut inner)?;
        let last_used = next_access_id(&mut inner)?;
        let record = inner
            .cursors
            .get_mut(&cursor_id)
            .ok_or(ProblemCursorError::Invalid)?;
        record.state = ProblemCursorState::Reserved(operation_id);
        let snapshot = inner
            .snapshots
            .get_mut(&handle_id)
            .ok_or(ProblemCursorError::Invalid)?;
        snapshot.in_flight = Some(operation_id);
        snapshot.last_used = last_used;
        snapshot.expires_at = Instant::now() + self.ttl;
        let snapshot_handle = self.snapshot_handle(handle_id, snapshot);
        Ok(ProblemSnapshotLease {
            snapshot_id,
            position,
            query,
            handle_id,
            operation_id,
            cursor_id: Some(cursor_id),
            snapshot_handle,
        })
    }

    /// Atomically commits one successfully materialized page and, when needed,
    /// replaces its reserved cursor with the only valid successor.
    pub fn commit_page(
        &self,
        lease: &ProblemSnapshotLease,
        next_position: Option<usize>,
    ) -> Result<Option<String>, ProblemCursorError> {
        let mut inner = self.lock_inner();
        let snapshot = inner
            .snapshots
            .get(&lease.handle_id)
            .ok_or(ProblemCursorError::Invalid)?;
        if snapshot.snapshot_id != lease.snapshot_id
            || snapshot.query != lease.query
            || snapshot.in_flight != Some(lease.operation_id)
        {
            return Err(ProblemCursorError::Invalid);
        }
        if let Some(cursor_id) = lease.cursor_id {
            let cursor = inner
                .cursors
                .get(&cursor_id)
                .ok_or(ProblemCursorError::Invalid)?;
            if cursor.state != ProblemCursorState::Reserved(lease.operation_id) {
                return Err(ProblemCursorError::Invalid);
            }
        }
        let analysis = snapshot.analysis;

        // Allocate and authenticate the successor before changing the reserved
        // cursor. Any failure leaves the reservation available for rollback.
        let successor = if let Some(position) = next_position {
            let cursor_id = next_registry_id(&mut inner)?;
            let record = ProblemCursorRecord {
                handle_id: lease.handle_id,
                snapshot_id: lease.snapshot_id,
                analysis,
                query: lease.query,
                position,
                state: ProblemCursorState::Fresh,
            };
            let token = format_capability("pc1", cursor_id, self.cursor_mac(cursor_id, &record));
            Some((cursor_id, record, token))
        } else {
            None
        };
        let last_used = next_access_id(&mut inner)?;

        if let Some(cursor_id) = lease.cursor_id {
            let consumed = inner
                .cursors
                .remove(&cursor_id)
                .ok_or(ProblemCursorError::Invalid)?;
            retire_cursor_locked(&mut inner, cursor_id, consumed);
        }
        let snapshot = inner
            .snapshots
            .get_mut(&lease.handle_id)
            .ok_or(ProblemCursorError::Invalid)?;
        snapshot.in_flight = None;
        snapshot.last_used = last_used;
        snapshot.expires_at = Instant::now() + self.ttl;
        if let Some((cursor_id, record, token)) = successor {
            inner.cursors.insert(cursor_id, record);
            Ok(Some(token))
        } else {
            Ok(None)
        }
    }

    /// Restores a reserved continuation to Fresh after any page/DTO/successor
    /// failure. Only the operation that owns the reservation can roll it back.
    pub fn rollback_page(&self, lease: &ProblemSnapshotLease) {
        let mut inner = self.lock_inner();
        if let Some(cursor_id) = lease.cursor_id {
            if let Some(cursor) = inner.cursors.get_mut(&cursor_id) {
                if cursor.state == ProblemCursorState::Reserved(lease.operation_id) {
                    cursor.state = ProblemCursorState::Fresh;
                }
            }
        }
        if let Some(snapshot) = inner.snapshots.get_mut(&lease.handle_id) {
            if snapshot.in_flight == Some(lease.operation_id) {
                snapshot.in_flight = None;
            }
        }
    }

    /// Abandons an unreturned first page and schedules its core snapshot for
    /// release. This also removes every cursor owned by the snapshot.
    pub fn abandon_page(&self, lease: &ProblemSnapshotLease) {
        let mut inner = self.lock_inner();
        if inner
            .snapshots
            .get(&lease.handle_id)
            .is_some_and(|snapshot| snapshot.in_flight == Some(lease.operation_id))
        {
            retire_snapshot_locked(
                &mut inner,
                lease.handle_id,
                RetiredSnapshotReason::Evicted,
                true,
            );
        }
    }

    pub fn resolve_snapshot_handle(
        &self,
        handle: &str,
        analysis: ProblemAnalysisIdentity,
    ) -> Result<ProblemSnapshotLease, ProblemCursorError> {
        let (handle_id, supplied_mac) = parse_capability(handle, "ph1")?;
        let mut inner = self.lock_inner();
        self.sweep_expired_locked(&mut inner, Instant::now());
        let Some(snapshot) = inner.snapshots.get(&handle_id) else {
            if let Some(retired) = inner.retired_snapshots.get(&handle_id) {
                let expected_mac = self.snapshot_mac(handle_id, &retired.snapshot);
                return if constant_time_eq(&supplied_mac, &expected_mac) {
                    Err(retired.reason.error())
                } else {
                    Err(ProblemCursorError::Tampered)
                };
            }
            return Err(ProblemCursorError::Invalid);
        };
        let expected_mac = self.snapshot_mac(handle_id, snapshot);
        if !constant_time_eq(&supplied_mac, &expected_mac) {
            return Err(ProblemCursorError::Tampered);
        }
        if snapshot.analysis != analysis {
            return Err(ProblemCursorError::AnalysisMismatch);
        }
        let snapshot_id = snapshot.snapshot_id;
        let query = snapshot.query;
        let last_used = next_access_id(&mut inner)?;
        let snapshot = inner
            .snapshots
            .get_mut(&handle_id)
            .ok_or(ProblemCursorError::Invalid)?;
        snapshot.last_used = last_used;
        snapshot.expires_at = Instant::now() + self.ttl;
        Ok(ProblemSnapshotLease {
            snapshot_id,
            position: 0,
            query,
            handle_id,
            operation_id: 0,
            cursor_id: None,
            snapshot_handle: handle.to_string(),
        })
    }

    pub fn mark_snapshot_released(&self, lease: &ProblemSnapshotLease) {
        let mut inner = self.lock_inner();
        if inner.snapshots.contains_key(&lease.handle_id) {
            retire_snapshot_locked(
                &mut inner,
                lease.handle_id,
                RetiredSnapshotReason::Released,
                false,
            );
        }
    }

    pub fn drain_core_releases(&self) -> Vec<u64> {
        std::mem::take(&mut self.lock_inner().pending_core_releases)
    }

    pub fn clear(&self) {
        *self.lock_inner() = ProblemCursorRegistryInner::default();
    }

    fn sweep_expired_locked(&self, inner: &mut ProblemCursorRegistryInner, now: Instant) {
        let expired = inner
            .snapshots
            .iter()
            .filter(|(_, snapshot)| snapshot.in_flight.is_none() && snapshot.expires_at <= now)
            .map(|(handle_id, _)| *handle_id)
            .collect::<Vec<_>>();
        for handle_id in expired {
            retire_snapshot_locked(inner, handle_id, RetiredSnapshotReason::Expired, true);
        }
    }

    fn snapshot_handle(&self, id: u64, record: &ProblemSnapshotRecord) -> String {
        format_capability("ph1", id, self.snapshot_mac(id, record))
    }

    fn snapshot_mac(&self, id: u64, record: &ProblemSnapshotRecord) -> [u64; 2] {
        self.mac_pair(
            "problem-snapshot-handle-v1",
            &(
                id,
                record.snapshot_id,
                record.analysis,
                record.query.signature(),
            ),
        )
    }

    fn cursor_mac(&self, id: u64, record: &ProblemCursorRecord) -> [u64; 2] {
        self.mac_pair(
            "problem-page-cursor-v1",
            &(
                id,
                record.handle_id,
                record.snapshot_id,
                record.analysis,
                record.query.signature(),
                record.position,
            ),
        )
    }

    fn mac_pair<T: Hash>(&self, domain: &str, value: &T) -> [u64; 2] {
        [
            keyed_hash(&self.mac_left, domain, value),
            keyed_hash(&self.mac_right, domain, value),
        ]
    }

    fn lock_inner(&self) -> MutexGuard<'_, ProblemCursorRegistryInner> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[cfg(test)]
    fn active_len(&self) -> usize {
        self.lock_inner().snapshots.len()
    }

    #[cfg(test)]
    fn retired_len(&self) -> usize {
        let inner = self.lock_inner();
        inner.retired_snapshots.len() + inner.retired_cursors.len()
    }

    #[cfg(test)]
    fn cursor_len(&self) -> usize {
        self.lock_inner().cursors.len()
    }
}

impl Default for ProblemCursorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn keyed_hash<T: Hash>(state: &RandomState, domain: &str, value: &T) -> u64 {
    let mut hasher = state.build_hasher();
    domain.hash(&mut hasher);
    value.hash(&mut hasher);
    hasher.finish()
}

fn next_registry_id(inner: &mut ProblemCursorRegistryInner) -> Result<u64, ProblemCursorError> {
    inner.next_id = inner
        .next_id
        .checked_add(1)
        .ok_or(ProblemCursorError::Capacity)?;
    Ok(inner.next_id)
}

fn next_access_id(inner: &mut ProblemCursorRegistryInner) -> Result<u64, ProblemCursorError> {
    inner.next_access = inner
        .next_access
        .checked_add(1)
        .ok_or(ProblemCursorError::Capacity)?;
    Ok(inner.next_access)
}

fn retire_snapshot_locked(
    inner: &mut ProblemCursorRegistryInner,
    handle_id: u64,
    reason: RetiredSnapshotReason,
    release_core: bool,
) {
    let Some(snapshot) = inner.snapshots.remove(&handle_id) else {
        return;
    };
    if release_core {
        inner.pending_core_releases.push(snapshot.snapshot_id);
    }
    inner
        .cursors
        .retain(|_, cursor| cursor.handle_id != handle_id);
    inner
        .retired_cursors
        .retain(|_, cursor| cursor.handle_id != handle_id);
    inner.retired_order.retain(|capability| match capability {
        RetiredCapability::Snapshot(id) => inner.retired_snapshots.contains_key(id),
        RetiredCapability::Cursor(id) => inner.retired_cursors.contains_key(id),
    });
    inner
        .retired_snapshots
        .insert(handle_id, RetiredSnapshotRecord { snapshot, reason });
    inner
        .retired_order
        .push_back(RetiredCapability::Snapshot(handle_id));
    trim_retired_capabilities_locked(inner);
}

fn retire_cursor_locked(
    inner: &mut ProblemCursorRegistryInner,
    cursor_id: u64,
    cursor: ProblemCursorRecord,
) {
    inner.retired_cursors.insert(cursor_id, cursor);
    inner
        .retired_order
        .push_back(RetiredCapability::Cursor(cursor_id));
    trim_retired_capabilities_locked(inner);
}

fn trim_retired_capabilities_locked(inner: &mut ProblemCursorRegistryInner) {
    while inner.retired_snapshots.len() + inner.retired_cursors.len()
        > MAX_RETIRED_PROBLEM_CAPABILITIES
    {
        match inner.retired_order.pop_front() {
            Some(RetiredCapability::Snapshot(id)) => {
                inner.retired_snapshots.remove(&id);
            }
            Some(RetiredCapability::Cursor(id)) => {
                inner.retired_cursors.remove(&id);
            }
            None => break,
        }
    }
}

fn format_capability(prefix: &str, id: u64, mac: [u64; 2]) -> String {
    format!("{prefix}-{id:016x}-{:016x}{:016x}", mac[0], mac[1])
}

fn parse_capability(value: &str, prefix: &str) -> Result<(u64, [u64; 2]), ProblemCursorError> {
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.len() != 3 || parts[0] != prefix || parts[1].len() != 16 || parts[2].len() != 32 {
        return Err(ProblemCursorError::Invalid);
    }
    let id = u64::from_str_radix(parts[1], 16).map_err(|_| ProblemCursorError::Invalid)?;
    let mac = parts[2];
    let left = u64::from_str_radix(&mac[..16], 16).map_err(|_| ProblemCursorError::Invalid)?;
    let right = u64::from_str_radix(&mac[16..], 16).map_err(|_| ProblemCursorError::Invalid)?;
    Ok((id, [left, right]))
}

fn constant_time_eq(left: &[u64; 2], right: &[u64; 2]) -> bool {
    ((left[0] ^ right[0]) | (left[1] ^ right[1])) == 0
}

/// 全局应用状态:当前会话 + 打开代号(generation)。
/// 代号用于让被后续 open 取代的旧索引线程自行退出。
#[derive(Clone)]
pub struct AppState {
    pub session: Arc<Mutex<Option<Session>>>,
    pub generation: Arc<AtomicU64>,
    pub analysis_generation: Arc<AtomicU64>,
    pub filter_input_revision: Arc<AtomicU64>,
    pub applied_filter_input_revision: Arc<AtomicU64>,
    pub filter_result_revision: Arc<AtomicU64>,
    pub search_input_revision: Arc<AtomicU64>,
    pub applied_search_input_revision: Arc<AtomicU64>,
    pub decode_revision: Arc<AtomicU64>,
    pub source_data_revision: Arc<AtomicU64>,
    pub filter_task_generation: Arc<AtomicU64>,
    pub search_task_generation: Arc<AtomicU64>,
    pub export_task_generation: Arc<AtomicU64>,
    pub stream_generation: Arc<AtomicU64>,
    pub stream: Arc<Mutex<StreamRuntime>>,
    pub stream_control: Arc<Mutex<()>>,
    pub config_control: Arc<Mutex<()>>,
    pub problem_cursors: Arc<ProblemCursorRegistry>,
}

#[derive(Default)]
pub struct StreamRuntime {
    pub task: Option<StreamTask>,
    pub last_request: Option<StreamRequestState>,
    pub lifecycle: StreamLifecycle,
    pub control_error: Option<String>,
    /// Whether the last transport owner was confirmed terminal. A failed wait
    /// must never be silently treated as a sealable input.
    pub eof_confirmed: bool,
    pub orphan_child: Option<Arc<Mutex<Child>>>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum StreamLifecycle {
    #[default]
    Stopped,
    Starting,
    Running,
    Pausing,
    Paused,
    Finishing,
    ControlError,
}

pub struct StreamTask {
    pub generation: u64,
    pub child: Arc<Mutex<Child>>,
    pub handle: JoinHandle<Result<(), String>>,
    pub serial: String,
}

#[derive(Debug, Clone)]
pub struct StreamRequestState {
    pub adb_path: PathBuf,
    pub requested_serial: Option<String>,
    pub buffers: Vec<String>,
    pub session_path: PathBuf,
    pub session_generation: u64,
    pub since_timestamp: Option<String>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            generation: Arc::new(AtomicU64::new(0)),
            analysis_generation: Arc::new(AtomicU64::new(0)),
            filter_input_revision: Arc::new(AtomicU64::new(0)),
            applied_filter_input_revision: Arc::new(AtomicU64::new(0)),
            filter_result_revision: Arc::new(AtomicU64::new(0)),
            search_input_revision: Arc::new(AtomicU64::new(0)),
            applied_search_input_revision: Arc::new(AtomicU64::new(0)),
            decode_revision: Arc::new(AtomicU64::new(0)),
            source_data_revision: Arc::new(AtomicU64::new(0)),
            filter_task_generation: Arc::new(AtomicU64::new(0)),
            search_task_generation: Arc::new(AtomicU64::new(0)),
            export_task_generation: Arc::new(AtomicU64::new(0)),
            stream_generation: Arc::new(AtomicU64::new(0)),
            stream: Arc::new(Mutex::new(StreamRuntime::default())),
            stream_control: Arc::new(Mutex::new(())),
            config_control: Arc::new(Mutex::new(())),
            problem_cursors: Arc::new(ProblemCursorRegistry::new()),
        }
    }

    pub fn lock_session(&self) -> MutexGuard<'_, Option<Session>> {
        Self::lock_session_arc(&self.session)
    }

    pub fn lock_config_control(&self) -> MutexGuard<'_, ()> {
        match self.config_control.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn lock_session_arc(
        session: &Arc<Mutex<Option<Session>>>,
    ) -> MutexGuard<'_, Option<Session>> {
        match session.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Publish a new input identity and its Session atomically with respect to all
    /// generation-checked readers. The generation is bumped before replacement
    /// while the Session mutex is held, so no new token can ever observe the old
    /// Session.
    pub fn replace_session(&self, session: Session) -> (u64, u64) {
        let mut guard = self.lock_session();
        let session_generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let analysis_generation = self.analysis_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.reset_dataset_revisions();
        self.cancel_derived_tasks_while_session_locked();
        self.problem_cursors.clear();
        *guard = Some(session);
        (session_generation, analysis_generation)
    }

    /// Invalidate and drop the current Session before a mapped source file is
    /// truncated. The returned generations identify the empty replacement slot.
    pub fn invalidate_session(&self) -> (u64, u64) {
        let mut guard = self.lock_session();
        let session_generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let analysis_generation = self.analysis_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.reset_dataset_revisions();
        self.cancel_derived_tasks_while_session_locked();
        self.problem_cursors.clear();
        *guard = None;
        (session_generation, analysis_generation)
    }

    pub fn install_session_if_current(&self, session_generation: u64, session: Session) -> bool {
        let mut guard = self.lock_session();
        if self.generation.load(Ordering::SeqCst) != session_generation {
            return false;
        }
        *guard = Some(session);
        true
    }

    /// Advance input identity for an in-place source reset (for example an
    /// externally truncated live capture). The caller must already hold the
    /// Session mutex, making the generation change atomic to checked readers.
    pub fn advance_input_identity_while_session_locked(&self) -> (u64, u64) {
        let session_generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let analysis_generation = self.analysis_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.reset_dataset_revisions();
        self.cancel_derived_tasks_while_session_locked();
        self.problem_cursors.clear();
        (session_generation, analysis_generation)
    }

    /// Reset identities and cancel derived scans while the Session mutex forms
    /// the publication boundary for a replacement input.
    fn reset_dataset_revisions(&self) {
        self.filter_input_revision.store(0, Ordering::SeqCst);
        self.applied_filter_input_revision
            .store(0, Ordering::SeqCst);
        self.filter_result_revision.store(0, Ordering::SeqCst);
        self.search_input_revision.store(0, Ordering::SeqCst);
        self.applied_search_input_revision
            .store(0, Ordering::SeqCst);
        self.decode_revision.store(0, Ordering::SeqCst);
        self.source_data_revision.store(0, Ordering::SeqCst);
    }

    fn cancel_derived_tasks_while_session_locked(&self) {
        self.filter_task_generation.fetch_add(1, Ordering::SeqCst);
        self.search_task_generation.fetch_add(1, Ordering::SeqCst);
    }

    /// The caller must hold the Session mutex so a filter request, its pending
    /// spec, and its task identity are published as one ordered transition.
    pub fn publish_filter_input_revision(&self, requested: Option<u64>) -> Result<u64, u64> {
        Self::publish_monotonic_revision(&self.filter_input_revision, requested)
    }

    /// The caller must hold the Session mutex through the result-vector swap.
    pub fn complete_filter_result(&self, input_revision: u64) -> u64 {
        self.applied_filter_input_revision
            .store(input_revision, Ordering::SeqCst);
        self.filter_result_revision.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// The caller must hold the Session mutex, mirroring filter publication.
    pub fn publish_search_input_revision(&self, requested: Option<u64>) -> Result<u64, u64> {
        Self::publish_monotonic_revision(&self.search_input_revision, requested)
    }

    /// The caller must hold the Session mutex through the search result swap.
    pub fn complete_search_result(&self, input_revision: u64) {
        self.applied_search_input_revision
            .store(input_revision, Ordering::SeqCst);
    }

    fn publish_monotonic_revision(
        revision: &AtomicU64,
        requested: Option<u64>,
    ) -> Result<u64, u64> {
        match requested {
            None => Ok(revision.fetch_add(1, Ordering::SeqCst) + 1),
            Some(requested) => loop {
                let current = revision.load(Ordering::SeqCst);
                // A client revision identifies one immutable request payload.
                // Reusing it, even with different content, must not cancel and
                // replace the task that already owns that identity.
                if requested <= current {
                    return Err(current);
                }
                if revision
                    .compare_exchange(current, requested, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    return Ok(requested);
                }
            },
        }
    }

    pub fn bump_filter_result_revision(&self) -> u64 {
        self.filter_result_revision.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn bump_decode_revision(&self) -> u64 {
        self.decode_revision.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn bump_source_data_revision(&self) -> u64 {
        self.source_data_revision.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Take an atomic snapshot of both identities while holding the Session lock.
    pub fn lock_session_with_generations(&self) -> (u64, u64, MutexGuard<'_, Option<Session>>) {
        let guard = self.lock_session();
        (
            self.generation.load(Ordering::SeqCst),
            self.analysis_generation.load(Ordering::SeqCst),
            guard,
        )
    }

    pub fn next_filter_task_generation(&self) -> u64 {
        self.filter_task_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn is_current_filter_task(&self, task_generation: u64) -> bool {
        self.filter_task_generation.load(Ordering::SeqCst) == task_generation
    }

    pub fn next_search_task_generation(&self) -> u64 {
        self.search_task_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn is_current_search_task(&self, task_generation: u64) -> bool {
        self.search_task_generation.load(Ordering::SeqCst) == task_generation
    }

    pub fn next_export_generation(&self) -> u64 {
        self.export_task_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn is_current_export(&self, generation: u64) -> bool {
        self.export_task_generation.load(Ordering::SeqCst) == generation
    }

    pub fn lock_stream(&self) -> MutexGuard<'_, StreamRuntime> {
        match self.stream.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn lock_stream_control(&self) -> MutexGuard<'_, ()> {
        match self.stream_control.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn next_analysis_generation(&self) -> u64 {
        self.problem_cursors.clear();
        self.analysis_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn current_analysis_generation(&self) -> u64 {
        self.analysis_generation.load(Ordering::SeqCst)
    }

    pub fn next_stream_generation(&self) -> u64 {
        self.stream_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn is_current_stream_task(&self, task_generation: u64) -> bool {
        self.stream_generation.load(Ordering::SeqCst) == task_generation
    }

    /// 持锁校验会话代号。依赖写侧顺序"先递增 generation、后替换 session"
    /// (open_file / start_logcat / reset_stream_session_file),
    /// 因此持锁后代号仍匹配 ⇒ guard 里就是该代号对应的会话。
    pub fn lock_session_if_current(
        &self,
        session_generation: u64,
    ) -> Option<MutexGuard<'_, Option<Session>>> {
        let guard = self.lock_session();
        if self.generation.load(Ordering::SeqCst) == session_generation {
            Some(guard)
        } else {
            None
        }
    }

    pub fn lock_analysis_if_current(
        &self,
        session_generation: u64,
        analysis_generation: u64,
    ) -> Option<MutexGuard<'_, Option<Session>>> {
        let guard = self.lock_session();
        if self.generation.load(Ordering::SeqCst) == session_generation
            && self.analysis_generation.load(Ordering::SeqCst) == analysis_generation
        {
            Some(guard)
        } else {
            None
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn recovers_poisoned_session_lock() {
        let state = AppState::new();
        let session = state.session.clone();
        let _ = std::thread::spawn(move || {
            let _guard = session.lock().unwrap();
            panic!("poison session lock for regression test");
        })
        .join();

        let guard = state.lock_session();
        assert!(guard.is_none());
    }

    #[test]
    fn scan_task_generations_cancel_stale_filter_and_search_work() {
        let state = AppState::new();

        let first_filter = state.next_filter_task_generation();
        let second_filter = state.next_filter_task_generation();
        assert!(!state.is_current_filter_task(first_filter));
        assert!(state.is_current_filter_task(second_filter));

        let first_search = state.next_search_task_generation();
        let second_search = state.next_search_task_generation();
        assert!(!state.is_current_search_task(first_search));
        assert!(state.is_current_search_task(second_search));
    }

    #[test]
    fn config_transactions_are_serialized_across_cloned_app_state() {
        let state = AppState::new();
        let first_transaction = state.lock_config_control();
        let contender = state.clone();
        let (attempted_tx, attempted_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            attempted_tx.send(()).unwrap();
            let _transaction = contender.lock_config_control();
            acquired_tx.send(()).unwrap();
        });

        attempted_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            acquired_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "a concurrent config transaction acquired the lock before its predecessor completed"
        );

        drop(first_transaction);
        acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn client_dataset_revisions_are_strictly_single_use() {
        let state = AppState::new();
        let _guard = state.lock_session();

        assert_eq!(state.publish_filter_input_revision(Some(4)), Ok(4));
        assert_eq!(state.publish_filter_input_revision(Some(4)), Err(4));
        assert_eq!(state.publish_filter_input_revision(Some(3)), Err(4));
        assert_eq!(state.publish_search_input_revision(Some(9)), Ok(9));
        assert_eq!(state.publish_search_input_revision(Some(9)), Err(9));
    }

    #[test]
    fn export_generations_cancel_stale_export_work() {
        let state = AppState::new();

        let first_export = state.next_export_generation();
        let second_export = state.next_export_generation();
        assert!(!state.is_current_export(first_export));
        assert!(state.is_current_export(second_export));
    }

    #[test]
    fn stream_task_generation_cancels_stale_work() {
        let state = AppState::new();

        let first = state.next_stream_generation();
        let second = state.next_stream_generation();

        assert!(!state.is_current_stream_task(first));
        assert!(state.is_current_stream_task(second));
    }

    #[test]
    fn lock_session_if_current_rejects_stale_generation() {
        let state = AppState::new();
        let first = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(state.lock_session_if_current(first).is_some());

        let second = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(state.lock_session_if_current(first).is_none());
        assert!(state.lock_session_if_current(second).is_some());
    }

    #[test]
    fn analysis_lock_requires_both_session_and_analysis_generations() {
        let state = AppState::new();
        let session = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let first = state.next_analysis_generation();
        assert!(state.lock_analysis_if_current(session, first).is_some());

        let second = state.next_analysis_generation();
        assert!(state.lock_analysis_if_current(session, first).is_none());
        assert!(state.lock_analysis_if_current(session, second).is_some());

        let replacement = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(state.lock_analysis_if_current(session, second).is_none());
        assert!(state
            .lock_analysis_if_current(replacement, second)
            .is_some());
    }

    #[test]
    fn opaque_problem_cursor_binds_position_query_and_single_use() {
        let registry = ProblemCursorRegistry::new();
        let analysis = ProblemAnalysisIdentity {
            session_generation: 7,
            analysis_generation: 2,
        };
        let query = ProblemPageQuery::Groups {
            kind: Some(2),
            sort: 1,
        };
        let lease = registry.register_snapshot(91, analysis, query).unwrap();
        let cursor = registry.commit_page(&lease, Some(100)).unwrap().unwrap();

        assert!(cursor.starts_with("pc1-"));
        assert!(!cursor.contains("000000000000005b"));
        let resolved = registry
            .reserve_cursor(&cursor, analysis, Some(query))
            .unwrap();
        assert_eq!(resolved.snapshot_id, 91);
        assert_eq!(resolved.position, 100);
        registry.commit_page(&resolved, None).unwrap();
        assert_eq!(
            registry.reserve_cursor(&cursor, analysis, Some(query)),
            Err(ProblemCursorError::Replayed)
        );
    }

    #[test]
    fn tampering_and_query_mismatch_do_not_consume_a_fresh_cursor() {
        let registry = ProblemCursorRegistry::new();
        let analysis = ProblemAnalysisIdentity {
            session_generation: 7,
            analysis_generation: 2,
        };
        let query = ProblemPageQuery::Occurrences { group_id: 3 };
        let lease = registry.register_snapshot(12, analysis, query).unwrap();
        let cursor = registry.commit_page(&lease, Some(40)).unwrap().unwrap();
        let mut tampered = cursor.clone().into_bytes();
        let last = tampered.last_mut().unwrap();
        *last = if *last == b'0' { b'1' } else { b'0' };
        let tampered = String::from_utf8(tampered).unwrap();
        assert_eq!(
            registry.reserve_cursor(&tampered, analysis, None),
            Err(ProblemCursorError::Tampered)
        );
        assert_eq!(
            registry.reserve_cursor(
                &cursor,
                analysis,
                Some(ProblemPageQuery::Occurrences { group_id: 4 })
            ),
            Err(ProblemCursorError::QueryMismatch)
        );
        let reserved = registry
            .reserve_cursor(&cursor, analysis, Some(query))
            .unwrap();
        assert_eq!(reserved.position, 40);
        registry.rollback_page(&reserved);
        assert!(registry
            .reserve_cursor(&cursor, analysis, Some(query))
            .is_ok());
    }

    #[test]
    fn opaque_snapshot_handle_is_independent_and_releasable() {
        let registry = ProblemCursorRegistry::new();
        let analysis = ProblemAnalysisIdentity {
            session_generation: 7,
            analysis_generation: 2,
        };
        let query = ProblemPageQuery::Groups {
            kind: None,
            sort: 0,
        };
        let lease = registry.register_snapshot(99, analysis, query).unwrap();
        let cursor = registry.commit_page(&lease, Some(100)).unwrap().unwrap();
        assert_eq!(registry.cursor_len(), 1);
        assert!(lease.snapshot_handle.starts_with("ph1-"));
        assert!(!lease.snapshot_handle.contains("0000000000000063"));
        let release = registry
            .resolve_snapshot_handle(&lease.snapshot_handle, analysis)
            .unwrap();
        assert_eq!(release.snapshot_id, 99);
        registry.mark_snapshot_released(&release);
        assert_eq!(registry.cursor_len(), 0);
        assert_eq!(
            registry.reserve_cursor(&cursor, analysis, Some(query)),
            Err(ProblemCursorError::Invalid)
        );
        assert_eq!(
            registry.resolve_snapshot_handle(&lease.snapshot_handle, analysis),
            Err(ProblemCursorError::Released)
        );
    }

    #[test]
    fn page_or_successor_failure_rolls_reserved_cursor_back_for_retry() {
        let registry = ProblemCursorRegistry::new();
        let analysis = ProblemAnalysisIdentity {
            session_generation: 7,
            analysis_generation: 2,
        };
        let query = ProblemPageQuery::Occurrences { group_id: 3 };
        let first = registry.register_snapshot(12, analysis, query).unwrap();
        let cursor = registry.commit_page(&first, Some(40)).unwrap().unwrap();
        let reserved = registry
            .reserve_cursor(&cursor, analysis, Some(query))
            .unwrap();
        let saved_next_id = {
            let mut inner = registry.lock_inner();
            let saved = inner.next_id;
            inner.next_id = u64::MAX;
            saved
        };
        assert_eq!(
            registry.commit_page(&reserved, Some(80)),
            Err(ProblemCursorError::Capacity)
        );
        registry.rollback_page(&reserved);
        registry.lock_inner().next_id = saved_next_id;

        let retried = registry
            .reserve_cursor(&cursor, analysis, Some(query))
            .unwrap();
        registry.commit_page(&retried, None).unwrap();
        assert_eq!(
            registry.reserve_cursor(&cursor, analysis, Some(query)),
            Err(ProblemCursorError::Replayed)
        );
    }

    #[test]
    fn concurrent_double_reserve_has_exactly_one_owner() {
        use std::sync::Barrier;

        let registry = Arc::new(ProblemCursorRegistry::new());
        let analysis = ProblemAnalysisIdentity {
            session_generation: 7,
            analysis_generation: 2,
        };
        let query = ProblemPageQuery::Groups {
            kind: None,
            sort: 0,
        };
        let first = registry.register_snapshot(91, analysis, query).unwrap();
        let cursor = registry.commit_page(&first, Some(100)).unwrap().unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let registry = registry.clone();
            let cursor = cursor.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                registry.reserve_cursor(&cursor, analysis, Some(query))
            }));
        }
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == Err(ProblemCursorError::InUse))
                .count(),
            1
        );
    }

    #[test]
    fn active_snapshots_are_lru_bounded_and_enqueue_core_release() {
        let registry = ProblemCursorRegistry::new();
        let analysis = ProblemAnalysisIdentity {
            session_generation: 7,
            analysis_generation: 2,
        };
        let query = ProblemPageQuery::Groups {
            kind: None,
            sort: 0,
        };
        for snapshot_id in 100..109 {
            let lease = registry
                .register_snapshot(snapshot_id, analysis, query)
                .unwrap();
            registry.commit_page(&lease, None).unwrap();
        }
        assert_eq!(registry.active_len(), MAX_ACTIVE_PROBLEM_SNAPSHOTS);
        assert_eq!(registry.drain_core_releases(), vec![100]);
    }

    #[test]
    fn ninth_registration_fails_when_all_active_pages_are_reserved() {
        let registry = ProblemCursorRegistry::new();
        let analysis = ProblemAnalysisIdentity {
            session_generation: 7,
            analysis_generation: 2,
        };
        let query = ProblemPageQuery::Groups {
            kind: None,
            sort: 0,
        };
        for snapshot_id in 1..=MAX_ACTIVE_PROBLEM_SNAPSHOTS as u64 {
            registry
                .register_snapshot(snapshot_id, analysis, query)
                .unwrap();
        }
        assert_eq!(
            registry.register_snapshot(9, analysis, query),
            Err(ProblemCursorError::Capacity)
        );
        assert_eq!(registry.active_len(), MAX_ACTIVE_PROBLEM_SNAPSHOTS);
        assert!(registry.drain_core_releases().is_empty());
    }

    #[test]
    fn ten_thousand_releases_keep_retired_and_cursor_state_bounded() {
        let registry = ProblemCursorRegistry::new();
        let analysis = ProblemAnalysisIdentity {
            session_generation: 7,
            analysis_generation: 2,
        };
        let query = ProblemPageQuery::Groups {
            kind: None,
            sort: 0,
        };
        for snapshot_id in 1..=10_000 {
            let lease = registry
                .register_snapshot(snapshot_id, analysis, query)
                .unwrap();
            registry.commit_page(&lease, None).unwrap();
            let release = registry
                .resolve_snapshot_handle(&lease.snapshot_handle, analysis)
                .unwrap();
            registry.mark_snapshot_released(&release);
        }
        assert_eq!(registry.active_len(), 0);
        assert!(registry.retired_len() <= MAX_RETIRED_PROBLEM_CAPABILITIES);
        assert_eq!(registry.cursor_len(), 0);
    }

    #[test]
    fn ten_thousand_expirations_release_core_and_clear_cursors() {
        let registry = ProblemCursorRegistry::with_ttl(Duration::ZERO);
        let analysis = ProblemAnalysisIdentity {
            session_generation: 7,
            analysis_generation: 2,
        };
        let query = ProblemPageQuery::Groups {
            kind: None,
            sort: 0,
        };
        let mut releases = 0;
        let mut last_handle = String::new();
        for snapshot_id in 1..=10_000 {
            let lease = registry
                .register_snapshot(snapshot_id, analysis, query)
                .unwrap();
            releases += registry.drain_core_releases().len();
            last_handle = lease.snapshot_handle.clone();
            registry.commit_page(&lease, Some(1)).unwrap();
        }
        assert_eq!(
            registry.resolve_snapshot_handle(&last_handle, analysis),
            Err(ProblemCursorError::Expired)
        );
        releases += registry.drain_core_releases().len();
        assert_eq!(releases, 10_000);
        assert_eq!(registry.active_len(), 0);
        assert!(registry.retired_len() <= MAX_RETIRED_PROBLEM_CAPABILITIES);
        assert_eq!(registry.cursor_len(), 0);
    }
}
