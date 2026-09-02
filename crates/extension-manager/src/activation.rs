use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard,
    atomic::{AtomicU64, Ordering},
};

use crate::ExtensionRoots;
use crate::error::{ErrorCode, ExtensionError, Result};
use crate::lifecycle;
use crate::model::{
    ActiveInstalledPackage, ExtensionInventory, InstalledPackageSummary, PackageHealth,
    PackageKind, PackageManifest, PackageReference, TrustReceipt,
};

const MAX_ACTIVE_CACHE_ENTRIES: usize = 16;
const SOFT_MAX_ACTIVE_CACHE_RETAINED_HANDLES: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CacheKey {
    base_root: PathBuf,
    package: PackageReference,
}

#[derive(Debug, Default)]
struct CacheState {
    entries: BTreeMap<CacheKey, CachedEntry>,
    busy: BTreeSet<CacheKey>,
    stats: ActivePackageCacheStats,
    retained_handles: usize,
    recency: u64,
}

#[derive(Debug)]
struct CachedEntry {
    active: ActiveInstalledPackage,
    retained_handles: usize,
    last_used: u64,
}

#[derive(Debug, Default)]
struct CacheShared {
    state: Mutex<CacheState>,
    ready: Condvar,
    full_hash_attempts: AtomicU64,
}

/// Process-local cache of exact active package leases.
///
/// The in-memory cache stores no additional persistent trust state. The common
/// lifecycle may maintain a receipt-bound Windows seal for large Codec trees;
/// callers must still explicitly invalidate an exact cache entry before an
/// operation that should release the cache-owned usage lease.
#[derive(Debug, Clone, Default)]
pub struct ActivePackageCache {
    shared: Arc<CacheShared>,
}

/// Observable validation work performed by one [`ActivePackageCache`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActivePackageCacheStats {
    pub full_hash_attempts: u64,
    pub cold_full_hash_passes: u64,
    pub persistent_fast_checkouts: u64,
    pub cached_checkouts: u64,
    pub capacity_evictions: u64,
    pub retained_entries: usize,
    pub retained_handles: usize,
}

impl ActivePackageCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve an enabled exact package, reusing its process-local active lease.
    ///
    /// # Errors
    ///
    /// Returns the same validation and lifecycle errors as [`lifecycle::resolve_active`].
    pub fn resolve_active(
        &self,
        roots: &ExtensionRoots,
        package: &PackageReference,
    ) -> Result<ActiveInstalledPackage> {
        let key = CacheKey {
            base_root: roots.base_root.clone(),
            package: package.clone(),
        };
        let cached = {
            let mut state = self.lock_state()?;
            while state.busy.contains(&key) {
                state = self.wait_state(state)?;
            }
            state.busy.insert(key.clone());
            state.recency = state.recency.saturating_add(1);
            let recency = state.recency;
            state.entries.get_mut(&key).map(|entry| {
                entry.last_used = recency;
                entry.active.clone()
            })
        };
        let (result, resolved_new_entry) = if let Some(active) = cached {
            #[cfg(windows)]
            {
                (
                    lifecycle::revalidate_cached_active(roots, package, &active).map(|()| active),
                    false,
                )
            }
            #[cfg(not(windows))]
            {
                drop(active);
                (
                    lifecycle::resolve_active_counted(
                        roots,
                        package,
                        &self.shared.full_hash_attempts,
                    ),
                    true,
                )
            }
        } else {
            (
                lifecycle::resolve_active_counted(roots, package, &self.shared.full_hash_attempts),
                true,
            )
        };
        let mut state = self.lock_state()?;
        state.busy.remove(&key);
        match &result {
            Ok(active) if resolved_new_entry => {
                state.insert(key, active.clone());
                if active.full_hash_passes() == 0 {
                    state.stats.persistent_fast_checkouts =
                        state.stats.persistent_fast_checkouts.saturating_add(1);
                } else {
                    state.stats.cold_full_hash_passes = state
                        .stats
                        .cold_full_hash_passes
                        .saturating_add(active.full_hash_passes());
                }
            }
            Ok(_) => {
                state.stats.cached_checkouts = state.stats.cached_checkouts.saturating_add(1);
            }
            Err(_) => {
                state.remove(&key);
            }
        }
        self.shared.ready.notify_all();
        result
    }

    /// Fully validate, enable, and retain one exact package in a single payload
    /// pass so the following inventory and runtime checkout reuse that proof.
    ///
    /// # Errors
    ///
    /// Returns a stable validation or lifecycle error without changing the
    /// enabled receipt when validation or activation fails.
    pub fn enable_and_prime(
        &self,
        roots: &ExtensionRoots,
        package: &PackageReference,
    ) -> Result<ActiveInstalledPackage> {
        let key = CacheKey {
            base_root: roots.base_root.clone(),
            package: package.clone(),
        };
        {
            let mut state = self.lock_state()?;
            while state.busy.contains(&key) {
                state = self.wait_state(state)?;
            }
            state.busy.insert(key.clone());
            state.remove(&key);
        }
        let result =
            lifecycle::enable_active_counted(roots, package, &self.shared.full_hash_attempts);
        let mut state = self.lock_state()?;
        state.busy.remove(&key);
        if let Ok(active) = &result {
            state.insert(key, active.clone());
            state.stats.cold_full_hash_passes = state
                .stats
                .cold_full_hash_passes
                .saturating_add(active.full_hash_passes());
        }
        self.shared.ready.notify_all();
        result
    }

    /// Revoke one exact package for future checkouts without rereading payload
    /// bytes, after dropping the cache-owned active lease.
    ///
    /// Caller-held lease clones remain valid point-in-time runtime sessions;
    /// every future cache checkout observes the disabled receipt.
    ///
    /// # Errors
    ///
    /// Returns a stable package, receipt, or lifecycle error.
    pub fn disable(
        &self,
        roots: &ExtensionRoots,
        package: &PackageReference,
    ) -> Result<TrustReceipt> {
        let key = CacheKey {
            base_root: roots.base_root.clone(),
            package: package.clone(),
        };
        {
            let mut state = self.lock_state()?;
            while state.busy.contains(&key) {
                state = self.wait_state(state)?;
            }
            state.busy.insert(key.clone());
            state.remove(&key);
        }
        let result = lifecycle::disable(roots, package);
        let mut state = self.lock_state()?;
        state.busy.remove(&key);
        self.shared.ready.notify_all();
        result
    }

    /// Build an authoritative installed-package snapshot while retaining a
    /// bounded working set of enabled healthy packages as active leases.
    ///
    /// The first Windows call performs one complete payload hash pass per
    /// enabled package. Later runtime boundaries reuse retained exact leases
    /// after receipt and closed-tree revalidation. A single schema-valid pack
    /// larger than the soft handle budget is admitted as the sole entry. Other
    /// platforms conservatively rehash cached payloads because retained read
    /// handles do not prevent a same-length in-place mutation there.
    ///
    /// # Errors
    ///
    /// Returns a stable root-level lifecycle error. Individual invalid package
    /// versions remain isolated in the returned inventory.
    pub fn runtime_inventory(&self, roots: &ExtensionRoots) -> Result<ExtensionInventory> {
        let (packages, manifests) =
            self.runtime_inventory_kinds(roots, &[PackageKind::DeckPack, PackageKind::CodecPack])?;
        let matrix = lifecycle::compatibility_matrix_from_inventory(&packages, &manifests);
        Ok(ExtensionInventory { packages, matrix })
    }

    /// List one package kind through the same active-lease cache used at
    /// runtime boundaries, without inspecting the other package root.
    ///
    /// # Errors
    ///
    /// Returns a stable root-level lifecycle error. Individual invalid package
    /// versions remain isolated in the returned summaries.
    pub fn runtime_list_kind(
        &self,
        roots: &ExtensionRoots,
        kind: PackageKind,
    ) -> Result<Vec<InstalledPackageSummary>> {
        self.runtime_inventory_kinds(roots, &[kind])
            .map(|(packages, _)| packages)
    }

    /// Drop the cache-owned lease for one exact package identity.
    ///
    /// Returns `true` when an entry was present. Caller-held clones remain
    /// active until their final clone is dropped.
    #[must_use]
    pub fn invalidate_exact(&self, roots: &ExtensionRoots, package: &PackageReference) -> bool {
        let key = CacheKey {
            base_root: roots.base_root.clone(),
            package: package.clone(),
        };
        let Ok(mut state) = self.shared.state.lock() else {
            return false;
        };
        while state.busy.contains(&key) {
            let Ok(waited) = self.shared.ready.wait(state) else {
                return false;
            };
            state = waited;
        }
        state.remove(&key).is_some()
    }

    /// Drop every cache-owned active package lease.
    pub fn invalidate_all(&self) {
        if let Ok(mut state) = self.shared.state.lock() {
            while !state.busy.is_empty() {
                let Ok(waited) = self.shared.ready.wait(state) else {
                    return;
                };
                state = waited;
            }
            state.entries.clear();
            state.retained_handles = 0;
        }
    }

    #[must_use]
    pub fn stats(&self) -> ActivePackageCacheStats {
        self.shared.state.lock().map_or_else(
            |_| ActivePackageCacheStats::default(),
            |state| ActivePackageCacheStats {
                full_hash_attempts: self.shared.full_hash_attempts.load(Ordering::Relaxed),
                retained_entries: state.entries.len(),
                retained_handles: state.retained_handles,
                ..state.stats
            },
        )
    }

    fn runtime_inventory_kinds(
        &self,
        roots: &ExtensionRoots,
        kinds: &[PackageKind],
    ) -> Result<(
        Vec<InstalledPackageSummary>,
        BTreeMap<PackageReference, PackageManifest>,
    )> {
        let mut candidates = lifecycle::discover_inventory_candidates(roots, kinds)?;
        candidates.sort_by(|left, right| {
            let left = left.package();
            let right = right.package();
            (
                left.kind.archive_extension(),
                &left.package_id,
                &left.package_version,
            )
                .cmp(&(
                    right.kind.archive_extension(),
                    &right.package_id,
                    &right.package_version,
                ))
        });
        let mut packages = Vec::with_capacity(candidates.len());
        let mut manifests = BTreeMap::new();
        for candidate in candidates {
            match candidate {
                lifecycle::InventoryCandidate::Exact {
                    package,
                    destination,
                } => {
                    let enabled =
                        match lifecycle::inventory_candidate_enabled(roots, &package, &destination)
                        {
                            Ok(enabled) => enabled,
                            Err(error) if is_root_inventory_error(&error) => return Err(error),
                            Err(_) => false,
                        };
                    if !enabled {
                        match summarize_inactive_candidate(roots, &package, &destination) {
                            Ok((summary, manifest)) => {
                                if let Some(manifest) = manifest {
                                    manifests.insert(package, manifest);
                                }
                                packages.push(summary);
                            }
                            Err(error) if is_root_inventory_error(&error) => return Err(error),
                            Err(error) => packages.push(isolated_package_error(package, &error)),
                        }
                        continue;
                    }
                    match self.resolve_active(roots, &package) {
                        Ok(active) => {
                            let manifest = active.manifest().clone();
                            packages.push(InstalledPackageSummary {
                                package: package.clone(),
                                display_name: Some(manifest.display_name().to_owned()),
                                publisher_name: Some(manifest.publisher().name.clone()),
                                enabled: true,
                                health: PackageHealth::Healthy,
                                error_code: None,
                                error_detail: None,
                            });
                            manifests.insert(package, manifest);
                        }
                        Err(error) if error.code() == ErrorCode::PackageDisabled => {
                            match summarize_inactive_candidate(roots, &package, &destination) {
                                Ok((summary, manifest)) => {
                                    if let Some(manifest) = manifest {
                                        manifests.insert(package, manifest);
                                    }
                                    packages.push(summary);
                                }
                                Err(error) if is_root_inventory_error(&error) => {
                                    return Err(error);
                                }
                                Err(error) => {
                                    packages.push(isolated_package_error(package, &error));
                                }
                            }
                        }
                        Err(error) if is_root_inventory_error(&error) => return Err(error),
                        Err(error) => packages.push(isolated_package_error(package, &error)),
                    }
                }
                lifecycle::InventoryCandidate::Isolated(summary) => packages.push(summary),
            }
        }
        packages.sort_by(|left, right| {
            (
                left.package.kind.archive_extension(),
                &left.package.package_id,
                &left.package.package_version,
            )
                .cmp(&(
                    right.package.kind.archive_extension(),
                    &right.package.package_id,
                    &right.package.package_version,
                ))
        });
        Ok((packages, manifests))
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, CacheState>> {
        self.shared.state.lock().map_err(|_| {
            ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "active package cache lock is poisoned",
            )
        })
    }

    fn wait_state<'a>(
        &self,
        state: MutexGuard<'a, CacheState>,
    ) -> Result<MutexGuard<'a, CacheState>> {
        self.shared.ready.wait(state).map_err(|_| {
            ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "active package cache lock is poisoned while waiting",
            )
        })
    }
}

impl CacheState {
    fn insert(&mut self, key: CacheKey, active: ActiveInstalledPackage) {
        let retained_handles = active.retained_handle_count();
        let refreshing_same_entry = self.entries.contains_key(&key);
        if !refreshing_same_entry
            && !requires_sole_entry(retained_handles)
            && self.retained_handles > SOFT_MAX_ACTIVE_CACHE_RETAINED_HANDLES
        {
            return;
        }
        self.remove(&key);
        // One schema-valid Codec Pack may legitimately exceed the soft cache
        // budget on its own. Keep that exact package as the sole entry so a
        // subsequent execution boundary does not immediately hash it again.
        if requires_sole_entry(retained_handles) {
            self.evict_all_for_capacity();
            self.insert_retained(key, active, retained_handles);
            return;
        }
        while self.entries.len() >= MAX_ACTIVE_CACHE_ENTRIES
            || self.retained_handles.saturating_add(retained_handles)
                > SOFT_MAX_ACTIVE_CACHE_RETAINED_HANDLES
        {
            let Some(evicted) = self
                .entries
                .iter()
                .min_by(|(left_key, left), (right_key, right)| {
                    (left.last_used, *left_key).cmp(&(right.last_used, *right_key))
                })
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.remove(&evicted);
            self.stats.capacity_evictions = self.stats.capacity_evictions.saturating_add(1);
        }
        self.insert_retained(key, active, retained_handles);
    }

    fn insert_retained(
        &mut self,
        key: CacheKey,
        active: ActiveInstalledPackage,
        retained_handles: usize,
    ) {
        self.recency = self.recency.saturating_add(1);
        self.retained_handles = self.retained_handles.saturating_add(retained_handles);
        self.entries.insert(
            key,
            CachedEntry {
                active,
                retained_handles,
                last_used: self.recency,
            },
        );
    }

    fn evict_all_for_capacity(&mut self) {
        let evicted = u64::try_from(self.entries.len()).unwrap_or(u64::MAX);
        self.entries.clear();
        self.retained_handles = 0;
        self.stats.capacity_evictions = self.stats.capacity_evictions.saturating_add(evicted);
    }

    fn remove(&mut self, key: &CacheKey) -> Option<CachedEntry> {
        let removed = self.entries.remove(key);
        if let Some(entry) = &removed {
            self.retained_handles = self.retained_handles.saturating_sub(entry.retained_handles);
        }
        removed
    }
}

const fn requires_sole_entry(retained_handles: usize) -> bool {
    retained_handles > SOFT_MAX_ACTIVE_CACHE_RETAINED_HANDLES
}

fn is_root_inventory_error(error: &ExtensionError) -> bool {
    error.code() == ErrorCode::LifecycleBusy
}

fn summarize_inactive_candidate(
    roots: &ExtensionRoots,
    package: &PackageReference,
    destination: &std::path::Path,
) -> Result<(InstalledPackageSummary, Option<PackageManifest>)> {
    if package.kind == PackageKind::CodecPack {
        lifecycle::summarize_disabled_codec_candidate(roots, package.clone(), destination)
    } else {
        lifecycle::summarize_inventory_candidate(roots, package.clone(), destination)
    }
}

fn isolated_package_error(
    package: PackageReference,
    error: &ExtensionError,
) -> InstalledPackageSummary {
    InstalledPackageSummary {
        package,
        display_name: None,
        publisher_name: None,
        enabled: false,
        health: if error.code() == ErrorCode::PackageUntrusted {
            PackageHealth::Untrusted
        } else {
            PackageHealth::Corrupt
        },
        error_code: Some(error.code().as_str().to_owned()),
        error_detail: Some(error.detail().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::{SOFT_MAX_ACTIVE_CACHE_RETAINED_HANDLES, requires_sole_entry};

    #[test]
    fn one_legal_over_budget_package_uses_the_sole_entry_policy() {
        assert!(!requires_sole_entry(SOFT_MAX_ACTIVE_CACHE_RETAINED_HANDLES));
        assert!(requires_sole_entry(
            SOFT_MAX_ACTIVE_CACHE_RETAINED_HANDLES + 1
        ));
        assert!(requires_sole_entry(32_768 + 2));
    }
}
