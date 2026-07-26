use std::sync::{Arc, Mutex};

pub const DEFAULT_PROBLEM_MEMORY_BUDGET_BYTES: usize = 112 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemMemoryBudgetError {
    ZeroLimit,
    LimitExceeded,
    SizeOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProblemMemoryStats {
    pub limit_bytes: usize,
    pub charged_bytes: usize,
    pub retained_heap_bytes: usize,
    pub high_water_charged_bytes: usize,
    pub denied_reservation_count: u64,
    pub limited: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProblemMemoryUsage {
    pub(crate) retained_bytes: usize,
    pub(crate) charged_bytes: usize,
}

impl ProblemMemoryUsage {
    pub(crate) fn checked_add(self, other: Self) -> Result<Self, ProblemMemoryBudgetError> {
        Ok(Self {
            retained_bytes: self
                .retained_bytes
                .checked_add(other.retained_bytes)
                .ok_or(ProblemMemoryBudgetError::SizeOverflow)?,
            charged_bytes: self
                .charged_bytes
                .checked_add(other.charged_bytes)
                .ok_or(ProblemMemoryBudgetError::SizeOverflow)?,
        })
    }
}

#[derive(Debug)]
struct BudgetState {
    limit_bytes: usize,
    charged_bytes: usize,
    retained_heap_bytes: usize,
    high_water_charged_bytes: usize,
    denied_reservation_count: u64,
    limited: bool,
}

#[derive(Debug, Clone)]
pub struct ProblemMemoryBudget {
    state: Arc<Mutex<BudgetState>>,
}

impl Default for ProblemMemoryBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl ProblemMemoryBudget {
    pub fn new() -> Self {
        Self::with_limit_bytes(DEFAULT_PROBLEM_MEMORY_BUDGET_BYTES)
            .expect("the documented Problems memory budget is non-zero")
    }

    pub fn with_limit_bytes(limit_bytes: usize) -> Result<Self, ProblemMemoryBudgetError> {
        if limit_bytes == 0 {
            return Err(ProblemMemoryBudgetError::ZeroLimit);
        }
        Ok(Self {
            state: Arc::new(Mutex::new(BudgetState {
                limit_bytes,
                charged_bytes: 0,
                retained_heap_bytes: 0,
                high_water_charged_bytes: 0,
                denied_reservation_count: 0,
                limited: false,
            })),
        })
    }

    pub fn stats(&self) -> ProblemMemoryStats {
        let state = self
            .state
            .lock()
            .expect("Problems memory budget mutex must not be poisoned");
        ProblemMemoryStats {
            limit_bytes: state.limit_bytes,
            charged_bytes: state.charged_bytes,
            retained_heap_bytes: state.retained_heap_bytes,
            high_water_charged_bytes: state.high_water_charged_bytes,
            denied_reservation_count: state.denied_reservation_count,
            limited: state.limited,
        }
    }

    pub(crate) fn account(&self) -> ProblemMemoryAccount {
        ProblemMemoryAccount {
            budget: self.clone(),
            usage: ProblemMemoryUsage::default(),
        }
    }

    pub(crate) fn clear_limit_state(&self) {
        let mut state = self
            .state
            .lock()
            .expect("Problems memory budget mutex must not be poisoned");
        state.high_water_charged_bytes = state.charged_bytes;
        state.denied_reservation_count = 0;
        state.limited = false;
    }
}

#[derive(Debug)]
pub(crate) struct ProblemMemoryAccount {
    budget: ProblemMemoryBudget,
    usage: ProblemMemoryUsage,
}

impl ProblemMemoryAccount {
    pub(crate) fn usage(&self) -> ProblemMemoryUsage {
        self.usage
    }

    pub(crate) fn try_set_usage(
        &mut self,
        usage: ProblemMemoryUsage,
    ) -> Result<(), ProblemMemoryBudgetError> {
        self.try_set_usage_inner(usage, true)
    }

    pub(crate) fn try_set_usage_transient(
        &mut self,
        usage: ProblemMemoryUsage,
    ) -> Result<(), ProblemMemoryBudgetError> {
        self.try_set_usage_inner(usage, false)
    }

    fn try_set_usage_inner(
        &mut self,
        mut usage: ProblemMemoryUsage,
        mark_limited: bool,
    ) -> Result<(), ProblemMemoryBudgetError> {
        usage.charged_bytes = usage.charged_bytes.max(usage.retained_bytes);
        let mut state = self
            .budget
            .state
            .lock()
            .expect("Problems memory budget mutex must not be poisoned");
        let other_charged = state
            .charged_bytes
            .checked_sub(self.usage.charged_bytes)
            .expect("an account cannot own more than the shared ledger");
        let next_charged = other_charged
            .checked_add(usage.charged_bytes)
            .ok_or(ProblemMemoryBudgetError::SizeOverflow)?;
        if next_charged > state.limit_bytes {
            if mark_limited {
                state.denied_reservation_count = state.denied_reservation_count.saturating_add(1);
                state.limited = true;
            }
            return Err(ProblemMemoryBudgetError::LimitExceeded);
        }
        let other_retained = state
            .retained_heap_bytes
            .checked_sub(self.usage.retained_bytes)
            .expect("an account cannot retain more than the shared ledger");
        let next_retained = other_retained
            .checked_add(usage.retained_bytes)
            .ok_or(ProblemMemoryBudgetError::SizeOverflow)?;
        state.charged_bytes = next_charged;
        state.retained_heap_bytes = next_retained;
        state.high_water_charged_bytes = state.high_water_charged_bytes.max(state.charged_bytes);
        self.usage = usage;
        Ok(())
    }

    pub(crate) fn settle_precharged(&mut self, mut usage: ProblemMemoryUsage) {
        usage.charged_bytes = usage.charged_bytes.max(usage.retained_bytes);
        debug_assert!(
            usage.charged_bytes <= self.usage.charged_bytes,
            "actual retained usage must fit inside its conservative precharge"
        );
        if usage.charged_bytes > self.usage.charged_bytes {
            let _ = self.try_set_usage(usage);
            return;
        }
        let mut state = self
            .budget
            .state
            .lock()
            .expect("Problems memory budget mutex must not be poisoned");
        state.charged_bytes = state
            .charged_bytes
            .checked_sub(self.usage.charged_bytes)
            .and_then(|bytes| bytes.checked_add(usage.charged_bytes))
            .expect("settling a precharge cannot overflow the ledger");
        state.retained_heap_bytes = state
            .retained_heap_bytes
            .checked_sub(self.usage.retained_bytes)
            .and_then(|bytes| bytes.checked_add(usage.retained_bytes))
            .expect("settling retained bytes cannot overflow the ledger");
        self.usage = usage;
    }

    pub(crate) fn release(&mut self) {
        self.settle_precharged(ProblemMemoryUsage::default());
    }
}

impl Drop for ProblemMemoryAccount {
    fn drop(&mut self) {
        if self.usage != ProblemMemoryUsage::default() {
            self.release();
        }
    }
}

pub(crate) fn vec_usage<T>(
    capacity: usize,
) -> Result<ProblemMemoryUsage, ProblemMemoryBudgetError> {
    aggregate_vec_usage::<T>(capacity, usize::from(capacity != 0))
}

pub(crate) fn aggregate_vec_usage<T>(
    total_capacity: usize,
    allocation_count: usize,
) -> Result<ProblemMemoryUsage, ProblemMemoryBudgetError> {
    let retained_bytes = total_capacity
        .checked_mul(std::mem::size_of::<T>())
        .ok_or(ProblemMemoryBudgetError::SizeOverflow)?;
    conservative_usage(retained_bytes, allocation_count)
}

pub(crate) fn vec_deque_usage<T>(
    capacity: usize,
) -> Result<ProblemMemoryUsage, ProblemMemoryBudgetError> {
    vec_usage::<T>(capacity)
}

pub(crate) fn hash_map_usage<K, V>(
    capacity: usize,
) -> Result<ProblemMemoryUsage, ProblemMemoryBudgetError> {
    let entry_bytes = std::mem::size_of::<K>()
        .checked_add(std::mem::size_of::<V>())
        .and_then(|bytes| bytes.checked_add(16))
        .ok_or(ProblemMemoryBudgetError::SizeOverflow)?;
    let retained_bytes = capacity
        .checked_mul(entry_bytes)
        .ok_or(ProblemMemoryBudgetError::SizeOverflow)?;
    conservative_usage(retained_bytes, usize::from(capacity != 0))
}

pub(crate) fn btree_map_usage<K, V>(
    len: usize,
) -> Result<ProblemMemoryUsage, ProblemMemoryBudgetError> {
    let entry_bytes = std::mem::size_of::<K>()
        .checked_add(std::mem::size_of::<V>())
        .and_then(|bytes| bytes.checked_add(4 * std::mem::size_of::<usize>()))
        .ok_or(ProblemMemoryBudgetError::SizeOverflow)?;
    let retained_bytes = len
        .checked_mul(entry_bytes)
        .ok_or(ProblemMemoryBudgetError::SizeOverflow)?;
    conservative_usage(retained_bytes, usize::from(len != 0))
}

fn conservative_usage(
    retained_bytes: usize,
    allocation_count: usize,
) -> Result<ProblemMemoryUsage, ProblemMemoryBudgetError> {
    if allocation_count == 0 {
        return Ok(ProblemMemoryUsage::default());
    }
    let allocator_overhead = retained_bytes
        .checked_div(4)
        .and_then(|overhead| {
            allocation_count
                .checked_mul(64)
                .and_then(|fixed| overhead.checked_add(fixed))
        })
        .ok_or(ProblemMemoryBudgetError::SizeOverflow)?;
    Ok(ProblemMemoryUsage {
        retained_bytes,
        charged_bytes: retained_bytes
            .checked_add(allocator_overhead)
            .ok_or(ProblemMemoryBudgetError::SizeOverflow)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_accounts_cannot_bypass_the_total_limit() {
        let budget = ProblemMemoryBudget::with_limit_bytes(1_000).unwrap();
        let mut index = budget.account();
        let mut correlation = budget.account();

        index
            .try_set_usage(ProblemMemoryUsage {
                retained_bytes: 400,
                charged_bytes: 600,
            })
            .unwrap();
        assert_eq!(
            correlation.try_set_usage(ProblemMemoryUsage {
                retained_bytes: 300,
                charged_bytes: 500,
            }),
            Err(ProblemMemoryBudgetError::LimitExceeded)
        );

        assert_eq!(index.usage().charged_bytes, 600);
        assert_eq!(correlation.usage().charged_bytes, 0);
        assert_eq!(budget.stats().charged_bytes, 600);
        assert_eq!(budget.stats().retained_heap_bytes, 400);
        assert_eq!(budget.stats().denied_reservation_count, 1);
        assert!(budget.stats().limited);
    }

    #[test]
    fn release_and_reset_reclaim_usage_and_limit_state() {
        let budget = ProblemMemoryBudget::with_limit_bytes(1_000).unwrap();
        let mut account = budget.account();
        account
            .try_set_usage(ProblemMemoryUsage {
                retained_bytes: 500,
                charged_bytes: 750,
            })
            .unwrap();
        assert!(account
            .try_set_usage(ProblemMemoryUsage {
                retained_bytes: 900,
                charged_bytes: 1_100,
            })
            .is_err());

        account.release();
        budget.clear_limit_state();

        assert_eq!(budget.stats().charged_bytes, 0);
        assert_eq!(budget.stats().retained_heap_bytes, 0);
        assert_eq!(budget.stats().high_water_charged_bytes, 0);
        assert_eq!(budget.stats().denied_reservation_count, 0);
        assert!(!budget.stats().limited);
    }

    #[test]
    fn conservative_container_charges_exceed_retained_capacity_bytes() {
        let vector = vec_usage::<u64>(100).unwrap();
        let map = hash_map_usage::<u32, u64>(100).unwrap();
        assert_eq!(vector.retained_bytes, 800);
        assert!(vector.charged_bytes > vector.retained_bytes);
        assert!(map.retained_bytes >= 100 * (4 + 8));
        assert!(map.charged_bytes > map.retained_bytes);
    }
}
