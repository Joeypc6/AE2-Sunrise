//! Capability providers over the read-only level (Sunrise main's `BlockCapabilityOffer`).
//!
//! A provider now answers from a `&dyn LevelHost` and hands over a constructor that runs without a
//! host. AE2's handlers are built from the server's state (the game's own container under them, a
//! part's config, the network's tunnels), so each is offered as a [`Deferred`] handler: the old
//! builder runs once, with the server host, at the handler's first use, and the built handler answers
//! every call after that (as the handler built at acquisition did). The provider decides presence
//! from what the read-only level shows, as the builder's own refusals did.

use mod_api::*;
use std::sync::OnceLock;

/// A server-side builder: what the provider used to return.
pub(crate) type Build<T> = fn(&mut dyn ServerHost, BlockSide) -> Option<Box<T>>;

/// A handler built on first use from `build` at `at`; one that refused answers as empty.
pub(crate) struct Deferred<T: ?Sized> {
    at: BlockSide,
    build: Build<T>,
    built: OnceLock<Option<Box<T>>>,
}

impl<T: ?Sized> Deferred<T> {
    pub(crate) fn new(at: BlockSide, build: Build<T>) -> Self {
        Self { at, build, built: OnceLock::new() }
    }

    fn get(&self, host: &mut dyn ServerHost) -> Option<&T> {
        if self.built.get().is_none() {
            let made = (self.build)(host, self.at);
            // A builder that asked this same handler again (it can't: it builds others) keeps the first.
            let _ = self.built.set(made);
        }
        self.built.get()?.as_deref()
    }
}

impl<R> ResourceHandler<R> for Deferred<dyn ResourceHandler<R>> {
    fn size(&self, host: &mut dyn ServerHost) -> usize {
        self.get(host).map_or(0, |h| h.size(host))
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<R> {
        self.get(host)?.resource(host, index)
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        self.get(host).map_or(0, |h| h.amount(host, index))
    }
    fn capacity(&self, host: &mut dyn ServerHost, index: usize, resource: Option<&R>) -> u32 {
        self.get(host).map_or(0, |h| h.capacity(host, index, resource))
    }
    fn is_valid(&self, host: &mut dyn ServerHost, index: usize, resource: &R) -> bool {
        self.get(host).is_some_and(|h| h.is_valid(host, index, resource))
    }
    fn insert_at(&self, host: &mut dyn ServerHost, tx: &Transaction, index: usize, resource: &R, amount: u32) -> u32 {
        self.get(host).map_or(0, |h| h.insert_at(host, tx, index, resource, amount))
    }
    fn extract_at(&self, host: &mut dyn ServerHost, tx: &Transaction, index: usize, resource: &R, amount: u32) -> u32 {
        self.get(host).map_or(0, |h| h.extract_at(host, tx, index, resource, amount))
    }
    fn insert(&self, host: &mut dyn ServerHost, tx: &Transaction, resource: &R, amount: u32) -> Result<u32, Overanswered> {
        match self.get(host) {
            Some(h) => h.insert(host, tx, resource, amount),
            None => Ok(0),
        }
    }
    fn extract(&self, host: &mut dyn ServerHost, tx: &Transaction, resource: &R, amount: u32) -> Result<u32, Overanswered> {
        match self.get(host) {
            Some(h) => h.extract(host, tx, resource, amount),
            None => Ok(0),
        }
    }
}

impl EnergyHandler for Deferred<dyn EnergyHandler> {
    fn amount(&self, host: &mut dyn ServerHost) -> u32 {
        self.get(host).map_or(0, |h| h.amount(host))
    }
    fn capacity(&self, host: &mut dyn ServerHost) -> u32 {
        self.get(host).map_or(0, |h| h.capacity(host))
    }
    fn insert(&self, host: &mut dyn ServerHost, tx: &Transaction, amount: u32) -> u32 {
        self.get(host).map_or(0, |h| h.insert(host, tx, amount))
    }
    fn extract(&self, host: &mut dyn ServerHost, tx: &Transaction, amount: u32) -> u32 {
        self.get(host).map_or(0, |h| h.extract(host, tx, amount))
    }
}

/// Whether the provider answers at all, from the read-only level.
pub(crate) type Present = fn(&dyn LevelHost, &BlockSide, BlockState, BlockEntityRef) -> bool;

/// Only asked through a side (asked with none, the game's own container answers).
pub(crate) fn sided(_: &dyn LevelHost, at: &BlockSide, _: BlockState, _: BlockEntityRef) -> bool {
    at.side.is_some()
}

/// Every side, and none.
pub(crate) fn always(_: &dyn LevelHost, _: &BlockSide, _: BlockState, _: BlockEntityRef) -> bool {
    true
}

/// Through a side, on a block entity with saved data (an interface's config).
pub(crate) fn sided_with_data(level: &dyn LevelHost, at: &BlockSide, _: BlockState, be: BlockEntityRef) -> bool {
    at.side.is_some() && level.block_entity_data(be).is_some()
}

/// The provider for `build` (ITEMS or FLUIDS), answering where `present` says.
pub(crate) fn items<R: 'static>(
    present: Present,
    build: Build<dyn ResourceHandler<R>>,
) -> impl Fn(&dyn LevelHost, BlockSide, BlockState, BlockEntityRef, &mut BlockCapabilityOffer<'_, dyn ResourceHandler<R>>) + Send + Sync + 'static {
    move |level, at, state, be, offer| {
        if present(level, &at, state, be) {
            offer.offer(|| Box::new(Deferred::new(at, build)));
        }
    }
}

/// The provider for `build` (ENERGY), answering where `present` says.
pub(crate) fn energy(
    present: Present,
    build: Build<dyn EnergyHandler>,
) -> impl Fn(&dyn LevelHost, BlockSide, BlockState, BlockEntityRef, &mut BlockCapabilityOffer<'_, dyn EnergyHandler>) + Send + Sync + 'static {
    move |level, at, state, be, offer| {
        if present(level, &at, state, be) {
            offer.offer(|| Box::new(Deferred::new(at, build)));
        }
    }
}

/// A block's `facing` property from its state (`ae2:x[facing=north,...]`).
pub(crate) fn state_facing(level: &dyn LevelHost, state: BlockState) -> Option<String> {
    let s = level.state_string(state)?;
    let props = s.split_once('[')?.1.trim_end_matches(']');
    props.split(',').find_map(|p| p.strip_prefix("facing=")).map(str::to_owned)
}
