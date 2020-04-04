// This file is part of radicle-link
// <https://github.com/radicle-dev/radicle-link>
//
// Copyright (C) 2019-2020 The Radicle Team <dev@radicle.xyz>
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License version 3 or
// later as published by the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

pub mod data;

use crate::{
    id::{uri::RadicleUri, user::User},
    keys::device::{Key, PublicKey, Signature},
};
use async_trait::async_trait;
use futures::stream::{Stream, StreamExt};
use multihash::{Multihash, Sha2_256};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    collections::{HashMap, HashSet},
    iter::FromIterator,
    str::FromStr,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Serialization failed ({0})")]
    SerializationFailed(serde_json::error::Error),

    #[error("Invalid UTF8 ({0})")]
    InvalidUtf8(std::string::FromUtf8Error),

    #[error("Invalid buffer encoding ({0})")]
    InvalidBufferEncoding(String),

    #[error("Invalid hash ({0})")]
    InvalidHash(String),

    #[error("Invalid URI ({0})")]
    InvalidUri(String),

    #[error("Signature already present ({0})")]
    SignatureAlreadyPresent(PublicKey),

    #[error("Invalid data ({0})")]
    InvalidData(String),

    #[error("Key not present ({0})")]
    KeyNotPresent(PublicKey),

    #[error("User not present ({0})")]
    UserNotPresent(RadicleUri),

    #[error("User key not present (uri {0}, key {1})")]
    UserKeyNotPresent(RadicleUri, PublicKey),

    #[error("Signature missing")]
    SignatureMissing,

    #[error("Signature decoding failed")]
    SignatureDecodingFailed,

    #[error("Signature verification failed")]
    SignatureVerificationFailed,

    #[error("Resolution failed (uri {0})")]
    ResolutionFailed(String),
}

#[derive(Debug, Error)]
pub enum UpdateVerificationError {
    #[error("Non monotonic revision")]
    NonMonotonicRevision,

    #[error("Wrong parent hash")]
    WrongParentHash,

    #[error("Update without previous quorum")]
    NoPreviousQuorum,

    #[error("Update without current quorum")]
    NoCurrentQuorum,
}

#[derive(Debug, Error)]
pub enum HistoryVerificationError {
    #[error("Empty history")]
    EmptyHistory,

    #[error("Error at revsion (rev {revision:?}, err {error:?})")]
    ErrorAtRevision { revision: u64, error: Error },

    #[error("Update error (rev {revision:?}, err {error:?})")]
    UpdateError {
        revision: u64,
        error: UpdateVerificationError,
    },
}

#[derive(Clone, Debug)]
pub enum Signatory {
    User(RadicleUri),
    OwnedKey,
}

#[derive(Clone, Debug)]
pub struct EntitySignature {
    pub by: Signatory,
    pub sig: Signature,
}

#[async_trait]
pub trait Resolver<T> {
    async fn resolve(&self, uri: &RadicleUri) -> Result<T, Error>;
}

// Sized
#[derive(Clone)]
pub struct Entity<T> {
    name: String,
    revision: u64,
    hash: Multihash,
    parent_hash: Option<Multihash>,
    signatures: HashMap<PublicKey, EntitySignature>,
    keys: HashSet<PublicKey>,
    certifiers: HashSet<RadicleUri>,
    info: T,
}

impl<T> Entity<T>
where
    T: Serialize + DeserializeOwned + Clone + Default,
{
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn from_data(data: data::EntityData<T>) -> Result<Self, Error> {
        if data.name.is_none() {
            return Err(Error::InvalidData("Missing name".to_owned()));
        }
        if data.revision.is_none() {
            return Err(Error::InvalidData("Missing revision".to_owned()));
        }
        if data.keys.is_empty() {
            return Err(Error::InvalidData("Missing keys".to_owned()));
        }

        let mut keys = HashSet::new();
        for k in data.keys.iter() {
            keys.insert(
                PublicKey::from_bs58(k).ok_or_else(|| Error::InvalidData(format!("key: {}", k)))?,
            );
        }

        let mut certifiers = HashSet::new();
        for c in data.certifiers.iter() {
            certifiers.insert(
                RadicleUri::from_str(c)
                    .map_err(|_| Error::InvalidData(format!("certifier: {}", c)))?,
            );
        }

        let mut signatures = HashMap::new();
        if let Some(s) = &data.signatures {
            for (k, sig) in s.iter() {
                let key = PublicKey::from_bs58(k)
                    .ok_or_else(|| Error::InvalidData(format!("signature key: {}", k)))?;
                let signature = EntitySignature {
                    by: match &sig.user {
                        Some(uri) => Signatory::User(RadicleUri::from_str(&uri)?),
                        None => Signatory::OwnedKey,
                    },
                    sig: Signature::from_bs58(&sig.sig).ok_or_else(|| {
                        Error::InvalidData(format!("signature data: {}", &sig.sig))
                    })?,
                };
                signatures.insert(key, signature);
            }
        }

        let actual_hash = data.compute_hash()?;
        if let Some(s) = &data.hash {
            let claimed_hash = {
                let bytes = bs58::decode(s.as_bytes())
                    .with_alphabet(bs58::alphabet::BITCOIN)
                    .into_vec()
                    .map_err(|_| Error::InvalidBufferEncoding(s.to_owned()))?;
                Multihash::from_bytes(bytes).map_err(|_| Error::InvalidHash(s.to_owned()))?
            };
            if claimed_hash != actual_hash {
                return Err(Error::InvalidHash(s.to_owned()));
            }
        }

        let parent_hash = match data.parent_hash {
            Some(s) => {
                let bytes = bs58::decode(s.as_bytes())
                    .with_alphabet(bs58::alphabet::BITCOIN)
                    .into_vec()
                    .map_err(|_| Error::InvalidBufferEncoding(s.to_owned()))?;
                let hash =
                    Multihash::from_bytes(bytes).map_err(|_| Error::InvalidHash(s.to_owned()))?;
                Some(hash)
            },
            None => None,
        };

        Ok(Self {
            name: data.name.unwrap(),
            revision: data.revision.unwrap().to_owned(),
            hash: actual_hash,
            parent_hash,
            keys,
            certifiers,
            signatures,
            info: data.info,
        })
    }

    pub fn to_data(&self) -> data::EntityData<T> {
        let mut signatures = HashMap::new();
        for (k, s) in self.signatures() {
            signatures.insert(
                k.to_bs58(),
                data::EntitySignatureData {
                    user: match &s.by {
                        Signatory::User(uri) => Some(uri.to_string()),
                        Signatory::OwnedKey => None,
                    },
                    sig: s.sig.to_bs58(),
                },
            );
        }

        let keys = HashSet::from_iter(self.keys().iter().map(|k| k.to_bs58()));
        let certifiers = HashSet::from_iter(self.certifiers().iter().map(|c| c.to_string()));

        data::EntityData {
            name: Some(self.name.to_owned()),
            revision: Some(self.revision),
            hash: Some(
                bs58::encode(&self.hash)
                    .with_alphabet(bs58::alphabet::BITCOIN)
                    .into_string(),
            ),
            parent_hash: self.parent_hash.to_owned().map(|h| {
                bs58::encode(h)
                    .with_alphabet(bs58::alphabet::BITCOIN)
                    .into_string()
            }),
            signatures: Some(signatures),
            keys,
            certifiers,
            info: self.info.to_owned(),
        }
    }

    pub fn to_builder(&self) -> data::EntityData<T> {
        self.to_data().clear_hash().clear_signatures()
    }

    pub fn hash(&self) -> &Multihash {
        &self.hash
    }
    pub fn uri(&self) -> RadicleUri {
        RadicleUri::new(self.hash.to_owned())
    }

    pub fn parent_hash(&self) -> &Option<Multihash> {
        &self.parent_hash
    }

    pub fn signatures(&self) -> &HashMap<PublicKey, EntitySignature> {
        &self.signatures
    }

    pub fn keys(&self) -> &HashSet<PublicKey> {
        &self.keys
    }
    fn keys_count(&self) -> usize {
        self.keys.len()
    }
    fn has_key(&self, key: &PublicKey) -> bool {
        self.keys.contains(key)
    }

    pub fn certifiers(&self) -> &HashSet<RadicleUri> {
        &self.certifiers
    }
    fn certifiers_count(&self) -> usize {
        self.certifiers.len()
    }
    fn has_certifier(&self, c: &RadicleUri) -> bool {
        self.certifiers.contains(c)
    }

    pub fn canonical_data(&self) -> Result<Vec<u8>, Error> {
        self.to_data().canonical_data()
    }
    pub fn to_json_writer<W>(&self, writer: W) -> Result<(), Error>
    where
        W: std::io::Write,
    {
        self.to_data().to_json_writer(writer)?;
        Ok(())
    }
    pub fn to_json_string(&self) -> Result<String, Error> {
        self.to_data().to_json_string()
    }

    pub fn from_json_reader<R>(r: R) -> Result<Self, Error>
    where
        R: std::io::Read,
    {
        Self::from_data(data::EntityData::from_json_reader(r)?)
    }
    pub fn from_json_str(s: &str) -> Result<Self, Error> {
        Self::from_data(data::EntityData::from_json_str(s)?)
    }

    pub fn compute_hash(&self) -> Result<Multihash, Error> {
        Ok(Sha2_256::digest(&self.canonical_data()?))
    }

    pub async fn check_key(
        &self,
        key: &PublicKey,
        by: &Signatory,
        resolver: &impl Resolver<User>,
    ) -> Result<(), Error> {
        match by {
            Signatory::OwnedKey => {
                if !self.has_key(key) {
                    return Err(Error::KeyNotPresent(key.to_owned()));
                }
            },
            Signatory::User(uri) => {
                let user = resolver.resolve(&uri).await?;
                if !user.has_key(key) {
                    return Err(Error::UserKeyNotPresent(uri.to_owned(), key.to_owned()));
                }
            },
        }
        Ok(())
    }

    pub fn compute_signature(&self, key: &Key) -> Result<Signature, Error> {
        Ok(key.sign(&self.canonical_data()?))
    }

    pub async fn sign(
        &mut self,
        key: &Key,
        by: &Signatory,
        resolver: &impl Resolver<User>,
    ) -> Result<(), Error> {
        let public_key = key.public();
        if self.signatures().contains_key(&public_key) {
            return Err(Error::SignatureAlreadyPresent(public_key.to_owned()));
        }
        self.check_key(&public_key, by, resolver).await?;
        let signature = EntitySignature {
            by: by.to_owned(),
            sig: self.compute_signature(key)?,
        };
        self.signatures.insert(public_key, signature);
        Ok(())
    }

    pub async fn check_signature(
        &self,
        key: &PublicKey,
        by: &Signatory,
        signature: &Signature,
        resolver: &impl Resolver<User>,
    ) -> Result<(), Error> {
        self.check_key(key, by, resolver).await?;
        if signature.verify(&self.canonical_data()?, key) {
            Ok(())
        } else {
            Err(Error::SignatureVerificationFailed)
        }
    }

    pub async fn check_validity(&self, resolver: &impl Resolver<User>) -> Result<(), Error> {
        let mut keys = HashSet::<PublicKey>::from_iter(self.keys().iter().cloned());
        let mut users = HashSet::<RadicleUri>::from_iter(self.certifiers().iter().cloned());

        for (k, s) in self.signatures() {
            self.check_signature(k, &s.by, &s.sig, resolver).await?;
            match &s.by {
                Signatory::OwnedKey => {
                    keys.remove(k);
                },
                Signatory::User(user) => {
                    users.remove(&user);
                },
            }
        }
        if keys.is_empty() && users.is_empty() {
            Ok(())
        } else {
            Err(Error::SignatureMissing)
        }
    }

    pub async fn is_valid(&self, resolver: &impl Resolver<User>) -> bool {
        self.check_validity(resolver).await.is_ok()
    }

    pub fn check_update(&self, previous: &Self) -> Result<(), UpdateVerificationError> {
        if self.revision() <= previous.revision() {
            return Err(UpdateVerificationError::NonMonotonicRevision);
        }

        match &self.parent_hash {
            Some(parent_hash) => {
                if &previous.hash != parent_hash {
                    return Err(UpdateVerificationError::WrongParentHash);
                }
            },
            None => {
                return Err(UpdateVerificationError::WrongParentHash);
            },
        }

        let retained_keys = self.keys().iter().filter(|k| previous.has_key(k)).count();
        let total_keys = self.keys_count();
        let added_keys = total_keys - retained_keys;
        let removed_keys = previous.keys_count() - retained_keys;
        let quorum_keys = total_keys / 2;

        if added_keys > quorum_keys {
            return Err(UpdateVerificationError::NoCurrentQuorum);
        } else if removed_keys > quorum_keys {
            return Err(UpdateVerificationError::NoPreviousQuorum);
        }

        let retained_certifiers = self
            .certifiers()
            .iter()
            .filter(|c| previous.has_certifier(c))
            .count();
        let total_certifiers = self.certifiers_count();
        let added_certifiers = total_certifiers - retained_certifiers;
        let removed_certifiers = previous.certifiers_count() - retained_certifiers;
        let quorum_certifiers = total_certifiers / 2;

        if added_certifiers > quorum_certifiers {
            return Err(UpdateVerificationError::NoCurrentQuorum);
        } else if removed_certifiers > quorum_certifiers {
            return Err(UpdateVerificationError::NoPreviousQuorum);
        }

        Ok(())
    }

    pub async fn check_history<R>(
        resolver: &impl Resolver<User>,
        revisions: R,
    ) -> Result<(), HistoryVerificationError>
    where
        R: Stream<Item = Entity<T>> + Unpin,
    {
        let mut revisions = revisions;
        let current = revisions.next().await;
        let mut current = match current {
            None => {
                return Err(HistoryVerificationError::EmptyHistory);
            },
            Some(entity) => entity,
        };

        let revision = current.revision();
        current
            .check_validity(resolver)
            .await
            .map_err(|error| HistoryVerificationError::ErrorAtRevision { revision, error })?;

        while let Some(previous) = revisions.next().await {
            let revision = current.revision();
            previous
                .check_validity(resolver)
                .await
                .map_err(|error| HistoryVerificationError::ErrorAtRevision { revision, error })?;
            current
                .check_update(&previous)
                .map_err(|error| HistoryVerificationError::UpdateError { revision, error })?;
            current = previous;
        }

        Ok(())
    }
}
