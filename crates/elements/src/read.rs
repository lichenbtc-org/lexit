//! A cursor over serialized bytes: Bitcoin's compact sizes and
//! length-prefixed strings.

pub(crate) struct Rd<'a> {
    pub(crate) b: &'a [u8],
    pub(crate) p: usize,
}

impl<'a> Rd<'a> {
    pub(crate) fn new(b: &'a [u8]) -> Rd<'a> {
        Rd { b, p: 0 }
    }
    pub(crate) fn done(&self) -> bool {
        self.p == self.b.len()
    }
    pub(crate) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.p.checked_add(n)?;
        let s = self.b.get(self.p..end)?;
        self.p = end;
        Some(s)
    }
    pub(crate) fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }
    pub(crate) fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub(crate) fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.array()?))
    }
    /// A compact size, at most what is left (each counted item is a byte
    /// at least).
    pub(crate) fn varint(&mut self) -> Option<usize> {
        let n = match self.u8()? {
            0xfd => u16::from_le_bytes(self.array()?) as u64,
            0xfe => u32::from_le_bytes(self.array()?) as u64,
            0xff => u64::from_le_bytes(self.array()?),
            n => n as u64,
        };
        let n = usize::try_from(n).ok()?;
        (n <= self.b.len() - self.p).then_some(n)
    }
    pub(crate) fn vbytes(&mut self) -> Option<&'a [u8]> {
        let n = self.varint()?;
        self.take(n)
    }
    /// A vector of byte strings (a witness stack).
    pub(crate) fn stack(&mut self) -> Option<alloc::vec::Vec<&'a [u8]>> {
        let n = self.varint()?;
        (0..n).map(|_| self.vbytes()).collect()
    }
}
