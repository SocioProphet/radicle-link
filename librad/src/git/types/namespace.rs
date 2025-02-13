// Copyright © 2019-2020 The Radicle Foundation <hello@radicle.foundation>
//
// This file is part of radicle-link, distributed under the GPLv3 with Radicle
// Linking Exception. For full terms see the included LICENSE file.

use std::fmt::{self, Display};

use git_ext as ext;
use multihash::Multihash;

use crate::{
    git::sealed,
    identities::urn::{self, Urn},
};

pub trait AsNamespace: Into<ext::RefLike> + sealed::Sealed {
    fn into_namespace(self) -> ext::RefLike {
        self.into()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Namespace<R>(Urn<R>);

impl<R> AsNamespace for Namespace<R>
where
    R: urn::HasProtocol,
    for<'a> &'a R: Into<Multihash>,
{
}

impl<'a, R> AsNamespace for &'a Namespace<R>
where
    R: urn::HasProtocol,
    &'a R: Into<Multihash>,
{
}

impl<R> sealed::Sealed for Namespace<R> {}
impl<R> sealed::Sealed for &Namespace<R> {}

impl<R> From<Urn<R>> for Namespace<R> {
    fn from(urn: Urn<R>) -> Self {
        Self(Urn { path: None, ..urn })
    }
}

impl<R: Clone> From<&Urn<R>> for Namespace<R> {
    fn from(urn: &Urn<R>) -> Self {
        Self(Urn {
            path: None,
            id: urn.id.clone(),
        })
    }
}

impl<R> Display for Namespace<R>
where
    R: urn::HasProtocol,
    for<'a> &'a R: Into<Multihash>,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(ext::RefLike::from(self).as_str())
    }
}

impl<R> From<Namespace<R>> for ext::RefLike
where
    R: urn::HasProtocol,
    for<'a> &'a R: Into<Multihash>,
{
    fn from(ns: Namespace<R>) -> Self {
        Self::from(ns.0)
    }
}

impl<'a, R> From<&'a Namespace<R>> for ext::RefLike
where
    R: urn::HasProtocol,
    &'a R: Into<Multihash>,
{
    fn from(ns: &'a Namespace<R>) -> Self {
        Self::from(&ns.0)
    }
}

impl<R> From<Namespace<R>> for ext::RefspecPattern
where
    R: urn::HasProtocol,
    for<'a> &'a R: Into<Multihash>,
{
    fn from(ns: Namespace<R>) -> Self {
        ext::RefLike::from(ns).into()
    }
}

impl<'a, R> From<&'a Namespace<R>> for ext::RefspecPattern
where
    R: urn::HasProtocol,
    &'a R: Into<Multihash>,
{
    fn from(ns: &'a Namespace<R>) -> Self {
        ext::RefLike::from(ns).into()
    }
}

impl<R> From<Namespace<R>> for Urn<R> {
    fn from(ns: Namespace<R>) -> Self {
        ns.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::identities::urn::tests::FakeId;

    #[test]
    fn is_reflike() {
        let ns = Namespace::from(Urn::new(ext::Oid::from(git2::Oid::zero())));
        assert_eq!(
            "hnrkyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy",
            ext::RefLike::from(ns).as_str()
        )
    }

    #[test]
    fn fake_is_reflike() {
        let ns = Namespace::from(Urn::new(FakeId(42)));
        assert_eq!("hyyryyyyyyyyyyybk", ext::RefLike::from(ns).as_str())
    }

    #[test]
    fn strips_path_from_urn() {
        let ns = Namespace::from(Urn {
            id: FakeId(42),
            path: Some(reflike!("lolek/bolek")),
        });
        assert_eq!("hyyryyyyyyyyyyybk", ext::RefLike::from(ns).as_str())
    }

    #[test]
    fn display_is_reflike_to_str() {
        let ns = Namespace::from(Urn::new(FakeId(69)));
        assert_eq!(&ns.to_string(), ext::RefLike::from(ns).as_str())
    }

    #[test]
    fn reflike_from_ref_from_owned() {
        let ns = Namespace::from(Urn::new(FakeId(666)));
        assert_eq!(ext::RefLike::from(&ns), ext::RefLike::from(ns))
    }
}
