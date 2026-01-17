use std::io::Read;

use lume_errors::Result;

use crate::{InputFileId, Object};

pub(crate) const MAGIC: [u8; 8] = *b"!<arch>\n";

pub(crate) fn is_archive(content: &[u8]) -> bool {
    content.starts_with(&MAGIC)
}

pub(crate) fn parse<D>(file: InputFileId, content: D) -> Result<Vec<Object>>
where
    D: AsRef<[u8]>,
{
    let mut archive = ar::Archive::new(content.as_ref());
    let mut objects = Vec::new();

    while let Some(entry) = archive.next_entry() {
        let mut entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                return Err(lume_errors::SimpleDiagnostic::new("could not parse archive entry")
                    .add_cause(err)
                    .into());
            }
        };

        let name = entry.header().identifier().to_vec();

        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;

        let entry_object = super::parse(file, &name, &buf)?;
        objects.extend(entry_object);
    }

    Ok(objects)
}
