//! Big-endian reading over a byte slice that never panics.

#[derive(Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub fn skip(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n);
    }

    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }

    pub fn u8(&mut self) -> Option<u8> {
        let v = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }

    pub fn i8(&mut self) -> Option<i8> {
        self.u8().map(|v| v as i8)
    }

    pub fn u16(&mut self) -> Option<u16> {
        let b = self.bytes(2)?;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }

    pub fn u24(&mut self) -> Option<u32> {
        let b = self.bytes(3)?;
        Some(u32::from_be_bytes([0, b[0], b[1], b[2]]))
    }

    pub fn u32(&mut self) -> Option<u32> {
        let b = self.bytes(4)?;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// An unsigned big-endian integer of 1 to 4 bytes.
    pub fn uint(&mut self, size: u8) -> Option<u32> {
        match size {
            1 => self.u8().map(u32::from),
            2 => self.u16().map(u32::from),
            3 => self.u24(),
            4 => self.u32(),
            _ => None,
        }
    }
}
