//! The statement's input's encoding (vlichen's): big-endian integers,
//! lists and byte strings with a 4-byte count.

use alloc::vec::Vec;

use crate::Reject;

pub(crate) struct W(pub(crate) Vec<u8>);

impl W {
    pub(crate) fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    pub(crate) fn list<const N: usize>(&mut self, items: &[[u8; N]]) {
        self.put(&(items.len() as u32).to_be_bytes());
        for i in items {
            self.put(i);
        }
    }
    pub(crate) fn bytes(&mut self, b: &[u8]) {
        self.put(&(b.len() as u32).to_be_bytes());
        self.put(b);
    }
}

pub(crate) struct R<'a>(pub(crate) &'a [u8]);

impl R<'_> {
    pub(crate) fn take<const N: usize>(&mut self) -> Result<[u8; N], Reject> {
        if self.0.len() < N {
            return Err(Reject::Encoding);
        }
        let (a, rest) = self.0.split_at(N);
        self.0 = rest;
        Ok(a.try_into().unwrap())
    }
    pub(crate) fn u8(&mut self) -> Result<u8, Reject> {
        Ok(self.take::<1>()?[0])
    }
    pub(crate) fn u32(&mut self) -> Result<u32, Reject> {
        Ok(u32::from_be_bytes(self.take()?))
    }
    pub(crate) fn list<const N: usize>(&mut self) -> Result<Vec<[u8; N]>, Reject> {
        let n = self.u32()? as usize;
        if n > self.0.len() / N {
            return Err(Reject::Encoding);
        }
        (0..n).map(|_| self.take()).collect()
    }
    pub(crate) fn bytes(&mut self) -> Result<Vec<u8>, Reject> {
        let n = self.u32()? as usize;
        if n > self.0.len() {
            return Err(Reject::Encoding);
        }
        let (a, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(a.to_vec())
    }
    pub(crate) fn end(&self) -> Result<(), Reject> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Reject::Encoding)
        }
    }
}
