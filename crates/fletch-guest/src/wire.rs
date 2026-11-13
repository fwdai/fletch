//! Reads the big-endian fields and length-prefixed blocks DNS and TLS messages
//! are made of. A read past the end is `None`.

pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, at: 0 }
    }

    /// How many bytes have been read.
    pub fn position(&self) -> usize {
        self.at
    }

    pub fn is_empty(&self) -> bool {
        self.at == self.bytes.len()
    }

    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let taken = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(taken)
    }

    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn u24(&mut self) -> Option<usize> {
        let b = self.take(3)?;
        Some(usize::from(b[0]) << 16 | usize::from(b[1]) << 8 | usize::from(b[2]))
    }

    /// A block led by its length in one byte.
    pub fn block8(&mut self) -> Option<&'a [u8]> {
        let len = self.u8()?;
        self.take(len.into())
    }

    /// A block led by its length in two bytes.
    pub fn block16(&mut self) -> Option<&'a [u8]> {
        let len = self.u16()?;
        self.take(len.into())
    }
}

#[cfg(test)]
mod tests {
    use super::Reader;

    #[test]
    fn fields_and_blocks_read_in_order() {
        let mut r = Reader::new(&[1, 0, 2, 0, 0, 3, 2, 7, 8, 0, 1, 9, 5]);
        assert_eq!(r.u8(), Some(1));
        assert_eq!(r.u16(), Some(2));
        assert_eq!(r.u24(), Some(3));
        assert_eq!(r.block8(), Some(&[7, 8][..]));
        assert_eq!(r.block16(), Some(&[9][..]));
        assert_eq!(r.position(), 12);
        assert!(!r.is_empty());
        assert_eq!(r.u16(), None);
        assert_eq!(r.u8(), Some(5));
        assert!(r.is_empty());
    }

    #[test]
    fn a_block_longer_than_what_is_left_is_none() {
        assert_eq!(Reader::new(&[3, 1, 2]).block8(), None);
        assert_eq!(Reader::new(&[0xff, 0xff, 1]).block16(), None);
    }
}
