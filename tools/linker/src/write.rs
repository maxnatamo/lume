use lume_errors::Result;

use crate::{Format, LayoutBuilder, Linker, macho};

pub(crate) fn write_to<W: Writer>(writer: &mut W, linker: &mut Linker) -> Result<()> {
    match linker.target.format {
        Format::MachO => {
            let print_entries = linker.config.print_entries;

            let mut builder = LayoutBuilder::<macho::Entry>::new(linker);
            macho::declare_layout(&mut builder);

            let layout = builder.into_layout();

            #[allow(clippy::disallowed_macros, reason = "used for non-logging purposes in the CLI")]
            if print_entries {
                println!("{layout}");
            }

            macho::emit_layout(writer, layout)
        }
        _ => unimplemented!(),
    }
}

/// Trait for writing data to a buffer (which may be a memory block, file
/// descriptor or otherwise).
pub(crate) trait Writer {
    /// Returns the current length of the writer.
    fn len(&self) -> usize;

    /// Writes the given bytes to the writer at the current position.
    fn write(&mut self, data: &[u8]) -> Result<()>;

    /// Writes the given byte to the writer at the current position.
    fn write_u8(&mut self, value: u8) -> Result<()> {
        self.write(&[value])
    }

    /// Writes the given 16-bit unsigned integer to the writer at the current
    /// position.
    fn write_u16(&mut self, value: u16) -> Result<()> {
        self.write(&value.to_ne_bytes())
    }

    /// Writes the given 32-bit unsigned integer to the writer at the current
    /// position.
    fn write_u32(&mut self, value: u32) -> Result<()> {
        self.write(&value.to_ne_bytes())
    }

    /// Writes the given 64-bit unsigned integer to the writer at the current
    /// position.
    fn write_u64(&mut self, value: u64) -> Result<()> {
        self.write(&value.to_ne_bytes())
    }

    /// Ensures the writer is aligned to the given alignment.
    fn align_to(&mut self, alignment: usize) -> Result<()> {
        let aligned = crate::align_to(self.len() as u64, alignment as u64);
        if aligned > self.len() as u64 {
            self.write(&vec![0; usize::try_from(aligned - self.len() as u64).unwrap()])?;
        }

        Ok(())
    }
}

pub(crate) struct MemoryWriter {
    // TODO: is there a more performant data type for this?
    data: Vec<u8>,
}

impl MemoryWriter {
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    pub fn into_inner(self) -> Box<[u8]> {
        self.data.into_boxed_slice()
    }
}

impl Writer for MemoryWriter {
    fn len(&self) -> usize {
        self.data.len()
    }

    fn write(&mut self, data: &[u8]) -> Result<()> {
        self.data.extend_from_slice(data);
        Ok(())
    }
}
