pub mod elf;
pub mod macho;

struct Patch<'data> {
    bytes: &'data mut [u8],
    offset: usize,
}

#[allow(dead_code)]
impl<'data> Patch<'data> {
    pub fn new(bytes: &'data mut [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    #[inline]
    pub fn skip(&mut self, n: isize) {
        self.offset = self.offset.strict_add_signed(n);
    }

    /// Reads the next `N` bytes without moving the current cursor.
    #[inline]
    pub fn peek_n<const N: usize>(&self, offset: isize) -> &[u8; N] {
        let off = self.offset.strict_add_signed(offset);
        let slice = &self.bytes[off..off + N];

        slice.try_into().unwrap()
    }

    /// Reads the next `u32` without moving the current cursor.
    #[inline]
    pub fn peek_u32(&self, offset: isize) -> u32 {
        u32::from_le_bytes(*self.peek_n::<4>(offset))
    }

    /// Reads the next `u64` without moving the current cursor.
    #[inline]
    pub fn peek_u64(&self, offset: isize) -> u64 {
        u64::from_le_bytes(*self.peek_n::<8>(offset))
    }

    /// Reads the next `N` bytes and moves the current cursor forward.
    #[inline]
    pub fn read_n<const N: usize>(&mut self, offset: isize) -> &[u8; N] {
        self.offset += N;
        let val = self.peek_n::<N>(offset.strict_sub_unsigned(N));

        val
    }

    /// Reads the next `u8` and moves the current cursor forward.
    #[inline]
    pub fn read_u8(&mut self, offset: isize) -> u8 {
        self.read_n::<1>(offset)[0]
    }

    /// Reads the next `u16` and moves the current cursor forward.
    #[inline]
    pub fn read_u16(&mut self, offset: isize) -> u16 {
        u16::from_le_bytes(*self.read_n::<2>(offset))
    }

    /// Reads the next `u32` and moves the current cursor forward.
    #[inline]
    pub fn read_u32(&mut self, offset: isize) -> u32 {
        u32::from_le_bytes(*self.read_n::<4>(offset))
    }

    /// Reads the next `u64` and moves the current cursor forward.
    #[inline]
    pub fn read_u64(&mut self, offset: isize) -> u64 {
        u64::from_le_bytes(*self.read_n::<8>(offset))
    }

    /// Writes the given value to the next `N` bytes and moves the current
    /// cursor forward.
    #[inline]
    pub fn write_n<const N: usize>(&mut self, offset: isize, value: &[u8; N]) {
        let off = self.offset.strict_add_signed(offset);

        self.offset += N;
        self.bytes[off..off + N].copy_from_slice(value);
    }

    /// Writes the given `u32` moves the current cursor forward.
    #[inline]
    pub fn write_u32(&mut self, offset: isize, value: u32) {
        self.write_n::<4>(offset, &value.to_le_bytes());
    }

    /// Writes the given `u64` moves the current cursor forward.
    #[inline]
    pub fn write_u64(&mut self, offset: isize, value: u64) {
        self.write_n::<8>(offset, &value.to_le_bytes());
    }
}
