// Copyright © 2019-2020 The Radicle Foundation <hello@radicle.foundation>
//
// This file is part of radicle-link, distributed under the GPLv3 with Radicle
// Linking Exception. For full terms see the included LICENSE file.

use std::{
    fmt::Debug,
    iter::{self, FromIterator},
    ops::Mul,
    sync::Arc,
};

use futures::channel::mpsc;
use parking_lot::RwLock;
use rand::seq::IteratorRandom as _;

use super::{
    error::Error,
    partial_view::{PartialView, Transition},
    periodic::{periodic_tasks, Periodic},
    rpc,
    Params,
    Tick,
};
use crate::{
    net::protocol::info::{PartialPeerInfo, PeerAdvertisement, PeerInfo},
    PeerId,
};

#[derive(Debug)]
pub struct Shuffle<Addr>
where
    Addr: Clone + Ord,
{
    pub recipient: PeerId,
    pub sample: Vec<PeerInfo<Addr>>,
    pub ttl: usize,
}

/// Watch me explode.
///
/// Return type for all state-transforming operations on [`Hpv`].
///
/// If you squint (hard!), this is kind of like a `ContT`: `ticks` contains
/// (defunctionalised) continuations to be interpreted by the caller, while
/// `trans` contains intermediate results. The intermediate results are of only
/// one type, indicating transitions on the partial view of the network the
/// respective operations yielded as side-effects.
#[derive(Debug)]
pub struct TnT<A>
where
    A: Clone + Ord,
{
    pub trans: Vec<Transition<A>>,
    pub ticks: Vec<Tick<A>>,
}

impl<A> TnT<A>
where
    A: Clone + Ord,
{
    pub fn with_tick(self, tick: impl Into<Option<Tick<A>>>) -> Self {
        Self {
            ticks: tick.into().into_iter().collect(),
            ..self
        }
    }
}

impl<A> Default for TnT<A>
where
    A: Clone + Ord,
{
    fn default() -> Self {
        Self {
            trans: Vec::default(),
            ticks: Vec::default(),
        }
    }
}

// `Default` + `Mul<Self>` = Monoid, innit?
impl<A> Mul<Self> for TnT<A>
where
    A: Clone + Ord,
{
    type Output = Self;

    fn mul(mut self, rhs: Self) -> Self {
        self.trans.extend(rhs.trans);
        self.ticks.extend(rhs.ticks);
        self
    }
}

impl<A> FromIterator<Transition<A>> for TnT<A>
where
    A: Clone + Ord,
{
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Transition<A>>,
    {
        let trans = iter.into_iter().collect::<Vec<_>>();
        let ticks = trans.iter().cloned().filter_map(Into::into).collect();
        Self { trans, ticks }
    }
}

impl<A> FromIterator<Tick<A>> for TnT<A>
where
    A: Clone + Ord,
{
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Tick<A>>,
    {
        Self {
            trans: Vec::default(),
            ticks: iter.into_iter().collect(),
        }
    }
}

impl<A> Extend<Tick<A>> for TnT<A>
where
    A: Clone + Ord,
{
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = Tick<A>>,
    {
        self.ticks.extend(iter)
    }
}

/// The classic [HyParView] membership protocol.
///
/// [HyParView]: https://asc.di.fct.unl.pt/~jleitao/pdf/dsn07-leitao.pdf
#[derive(Clone)]
pub struct Hpv<Rng, Addr>(Arc<RwLock<HpvInner<Rng, Addr>>>)
where
    Addr: Clone + Ord;

impl<Rng, Addr> Hpv<Rng, Addr>
where
    Rng: rand::Rng + Clone,
    Addr: Clone + Debug + Ord,
{
    pub fn new(local_id: PeerId, rng: Rng, params: Params) -> (Self, mpsc::Receiver<Periodic<Addr>>)
    where
        Rng: Send + Sync + 'static,
        Addr: Send + Sync + 'static,
    {
        let this = Self(Arc::new(RwLock::new(HpvInner::new(local_id, rng, params))));
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(periodic_tasks(this.clone(), tx));

        (this, rx)
    }

    pub fn view_stats(&self) -> (usize, usize) {
        let guard = self.0.read();
        (guard.num_active(), guard.num_passive())
    }

    pub fn is_active(&self, peer: &PeerId) -> bool {
        self.0.read().is_active(peer)
    }

    pub fn known(&self) -> Vec<PeerId> {
        self.0.read().known().collect()
    }

    #[tracing::instrument(level = "debug", skip(self))]
    #[must_use = "ticks must be interpreted"]
    pub fn connection_lost(&self, remote_peer: PeerId) -> TnT<Addr> {
        self.0.write().connection_lost(remote_peer)
    }

    #[tracing::instrument(level = "debug", skip(self))]
    #[must_use = "ticks must be interpreted"]
    pub fn connection_established(&self, info: PartialPeerInfo<Addr>) -> TnT<Addr> {
        self.0.write().connection_established(info)
    }

    #[must_use = "shuffles must be dispatched"]
    pub(super) fn shuffle(&self) -> Option<Shuffle<Addr>> {
        self.0.write().shuffle()
    }

    pub(super) fn choose_passive_to_promote(&self) -> Vec<PeerInfo<Addr>> {
        self.0.write().choose_passive_to_promote()
    }

    pub fn broadcast_recipients(&self, exclude: impl Into<Option<PeerId>>) -> Vec<PeerId> {
        self.0.read().broadcast_recipients(exclude.into())
    }

    #[tracing::instrument(skip(self))]
    #[must_use = "ticks must be interpreted"]
    pub fn apply(
        &self,
        remote_peer: PeerId,
        remote_addr: Addr,
        rpc: rpc::Message<Addr>,
    ) -> Result<TnT<Addr>, Error> {
        self.0.write().apply(remote_peer, remote_addr, rpc)
    }

    pub fn hello(&self, local_info: PeerAdvertisement<Addr>) -> rpc::Message<Addr> {
        use rpc::Message::*;

        match self.view_stats() {
            (0, 0) => Join { info: local_info },
            (act, _) => Neighbour {
                info: local_info,
                need_friends: (act == 0).then_some(()),
            },
        }
    }

    pub(super) fn params(&self) -> Params {
        self.0.read().params.clone()
    }
}

struct HpvInner<Rng, Addr>
where
    Addr: Clone + Ord,
{
    local_id: PeerId,
    params: Params,
    rng: Rng,
    view: PartialView<Rng, Addr>,
}

impl<Rng, Addr> HpvInner<Rng, Addr>
where
    Rng: rand::Rng + Clone,
    Addr: Clone + Debug + Ord,
{
    pub fn new(local_id: PeerId, rng: Rng, params: Params) -> Self {
        let view = PartialView::new(local_id, rng.clone(), params.max_active, params.max_passive);
        Self {
            local_id,
            params: Default::default(),
            rng,
            view,
        }
    }

    pub fn known(&self) -> impl Iterator<Item = PeerId> + '_ {
        self.view.known()
    }

    pub fn num_active(&self) -> usize {
        self.view.num_active()
    }

    pub fn num_passive(&self) -> usize {
        self.view.num_passive()
    }

    pub fn is_active(&self, peer: &PeerId) -> bool {
        self.view.is_active(peer)
    }

    pub fn connection_lost(&mut self, remote_peer: PeerId) -> TnT<Addr> {
        use Tick::*;

        tracing::debug!("connection lost");
        let mut tnt = self
            .view
            .demote(&remote_peer)
            .into_iter()
            .collect::<TnT<_>>();
        tnt.extend(
            self.choose_passive_to_promote()
                .into_iter()
                .map(|to| Connect { to }),
        );
        tnt
    }

    pub fn connection_established(&mut self, info: PartialPeerInfo<Addr>) -> TnT<Addr> {
        tracing::debug!("connection established");
        self.view.add_active(info).into_iter().collect()
    }

    pub fn shuffle(&mut self) -> Option<Shuffle<Addr>> {
        self.random_active().and_then(|recipient| {
            let sample = self
                .sample(self.params.shuffle_sample_size)
                .filter(|info| info.peer_id != recipient)
                .collect::<Vec<_>>();
            if sample.is_empty() {
                None
            } else {
                Some(Shuffle {
                    recipient,
                    sample,
                    ttl: self.params.active_random_walk_length,
                })
            }
        })
    }

    pub fn choose_passive_to_promote(&mut self) -> Vec<PeerInfo<Addr>> {
        let n = self.params.max_active - self.num_active();
        self.view.passive_info().choose_multiple(&mut self.rng, n)
    }

    pub fn broadcast_recipients(&self, exclude: Option<PeerId>) -> Vec<PeerId> {
        self.view
            .active()
            .filter(|peer_id| exclude.as_ref().map(|ex| ex != peer_id).unwrap_or(true))
            .collect()
    }

    pub fn apply(
        &mut self,
        remote_peer: PeerId,
        remote_addr: Addr,
        rpc: rpc::Message<Addr>,
    ) -> Result<TnT<Addr>, Error> {
        use rpc::Message::*;
        use Tick::*;

        tracing::debug!(
            active = self.num_active(),
            passive = self.num_passive(),
            "enter"
        );

        let res = match rpc {
            Join { .. } if self.is_active(&remote_peer) => Err(Error::JoinWhileConnected),
            Join { info } => {
                let info = peer_info_from(remote_peer, info, remote_addr);

                let mut tnt = self
                    .view
                    .add_active(info.clone().into())
                    .into_iter()
                    .collect::<TnT<_>>();
                let fwd = self.broadcast_recipients(Some(remote_peer));
                if !fwd.is_empty() {
                    tnt.ticks.push(All {
                        recipients: fwd,
                        message: ForwardJoin {
                            joined: info,
                            ttl: self.params.active_random_walk_length,
                        },
                    })
                }

                Ok(tnt)
            },

            ForwardJoin { joined, ttl }
                if (ttl == 0 || !self.view.is_active_full())
                    && !self.view.is_active(&joined.peer_id)
                    && joined.peer_id != self.local_id =>
            {
                Ok(TnT::default().with_tick(Connect { to: joined }))
            },
            ForwardJoin { joined, ttl } => {
                let mut tnt = if ttl == 0 {
                    self.view.add_passive(joined.clone()).into_iter().collect()
                } else {
                    TnT::default()
                };

                tnt.extend(
                    self.view
                        .active()
                        .filter(|peer| peer != &remote_peer)
                        .choose(&mut self.rng)
                        .map(|next_hop| All {
                            recipients: vec![next_hop],
                            message: ForwardJoin {
                                joined,
                                ttl: ttl.saturating_sub(1),
                            },
                        }),
                );

                Ok(tnt)
            },

            Neighbour { info, need_friends } => {
                if need_friends.is_some() || !self.view.is_active_full() {
                    let info = peer_info_from(remote_peer, info, remote_addr);
                    Ok(self.view.add_active(info.into()).into_iter().collect())
                } else {
                    Ok(TnT::default().with_tick(Reply {
                        to: remote_peer,
                        message: Disconnect,
                    }))
                }
            },

            Disconnect => Ok(self.view.demote(&remote_peer).into_iter().collect()),

            Shuffle { origin, peers, ttl } if ttl == 0 && origin.peer_id != self.local_id => {
                let sample = self.sample(peers.len()).collect::<Vec<_>>();
                let tnt = if !sample.is_empty() {
                    iter::once(Try {
                        recipient: origin,
                        message: ShuffleReply { peers: sample },
                    })
                    .collect()
                } else {
                    TnT::default()
                };

                Ok(peers.into_iter().fold(tnt, |acc, info| {
                    acc * self.view.add_passive(info).into_iter().collect()
                }))
            },
            Shuffle {
                mut origin,
                peers,
                ttl,
            } if ttl > 0 => {
                if origin.peer_id == remote_peer {
                    origin.seen_addrs.insert(remote_addr);
                }

                let tick = self
                    .view
                    .active()
                    .filter(|peer| peer != &remote_peer)
                    .choose(&mut self.rng)
                    .map(|next_hop| All {
                        recipients: vec![next_hop],
                        message: Shuffle {
                            origin,
                            peers,
                            ttl: ttl.saturating_sub(1),
                        },
                    });

                Ok(tick.into_iter().collect())
            },
            // TTL expired
            Shuffle { .. } => Ok(TnT::default()),

            ShuffleReply { peers } => Ok(peers.into_iter().fold(TnT::default(), |acc, info| {
                acc * self.view.add_passive(info).into_iter().collect()
            })),
        };

        tracing::debug!(
            active = self.num_active(),
            passive = self.num_passive(),
            "exit"
        );
        tracing::trace!("out: {:?}", res);

        res
    }

    fn random_active(&mut self) -> Option<PeerId> {
        self.view.active().choose(&mut self.rng)
    }

    fn sample(&mut self, sz: usize) -> impl Iterator<Item = PeerInfo<Addr>> + '_ {
        let mut sample = self
            .view
            .active_info()
            .filter_map(|info| info.sequence())
            .choose_multiple(&mut self.rng, sz);
        if sample.len() < self.params.shuffle_sample_size {
            sample.extend(
                self.view
                    .passive_info()
                    .choose_multiple(&mut self.rng, sz.saturating_sub(sample.len())),
            );
        }

        sample.into_iter()
    }
}

fn peer_info_from<Addr>(
    remote_peer: PeerId,
    advertised: PeerAdvertisement<Addr>,
    remote_addr: Addr,
) -> PeerInfo<Addr>
where
    Addr: Clone + Ord,
{
    PeerInfo {
        peer_id: remote_peer,
        advertised_info: advertised,
        seen_addrs: [remote_addr].iter().cloned().collect(),
    }
}
