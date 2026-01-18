use indexmap::{IndexMap, IndexSet};
use lume_errors::{Error, Result, SimpleDiagnostic};

use crate::Linker;
use crate::common::*;

#[derive(Default)]
pub(crate) struct Index {
    pub(crate) symbols: IndexMap<String, SymbolId>,
    pub(crate) dynamic_symbols: IndexMap<String, LibraryId>,

    pub(crate) sections: IndexMap<SectionName, SectionId>,
}

impl Index {
    /// Gets the symbol with the given name, if it exists.
    pub(crate) fn symbol_with_name(&self, name: &str) -> Option<SymbolId> {
        self.symbols.get(name).copied()
    }
}

impl Linker {
    /// Indexes all the symbols and sections within the linker and keys them by
    /// name.
    ///
    /// # Errors
    ///
    /// This method returns an error if there are duplicate symbols or
    /// unresolved symbols within the linker.
    pub fn index_symbols(&mut self) -> Result<()> {
        let mut symbols = Symbols::default();

        for symbol in self.db().symbols() {
            match symbol.linkage {
                Linkage::Global => symbols.add_global(symbol),
                Linkage::Local => symbols.add_local(symbol),
                Linkage::External => symbols.add_reference(symbol),
            }
        }

        for symbol in self.db().dynamic_symbols() {
            symbols.add_dynamic(symbol);
        }

        symbols.ensure_no_duplicates(self)?;
        symbols.ensure_no_unresolved(self)?;
        symbols.remove_unused_symbols();

        let globals = symbols
            .globals
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<Vec<_>>();

        let dynamics = symbols
            .dynamic
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<Vec<_>>();

        for (name, symbol) in globals {
            self.index.symbols.insert(name.clone(), symbol);
        }

        for (name, library) in dynamics {
            self.index.dynamic_symbols.insert(name.clone(), library);
        }

        for objects in self.db.objects.values() {
            for section in objects.sections.values() {
                let key = SectionName {
                    segment: section.segment.clone(),
                    section: section.name.clone(),
                };

                self.index.sections.insert(key, section.id);
            }
        }

        Ok(())
    }

    /// Merge all sections with the same section names into single sections.
    pub fn merge_sections(&mut self) {
        let mut segments = IndexMap::<String, IndexSet<MergedSectionId>>::new();
        let mut sections = IndexMap::<MergedSectionId, MergedSection>::new();

        for section in self.db().sections() {
            let id = MergedSectionId::from_name(section.segment.as_deref(), &section.name);

            if let Some(segment_name) = section.segment.clone() {
                segments.entry(segment_name).or_default().insert(id);
            }

            let merged_section = sections.entry(id).or_insert_with(|| MergedSection {
                id,
                name: SectionName {
                    segment: section.segment.clone(),
                    section: section.name.clone(),
                },
                placement: section.placement,
                size: 0,
                alignment: 0,
                kind: section.kind,
                merged_from: IndexSet::new(),
            });

            merged_section.size += section.data.len() as u64;
            merged_section.alignment = merged_section.alignment.max(section.alignment);
            merged_section.merged_from.insert(section.id);
        }

        self.db.merged_segments = segments;
        self.db.merged_sections = sections;
    }

    /// Adds the page zero segment to the segment mapping (only on macOS).
    pub fn add_pagezero_segment(&mut self) {
        if !self.target.has_page_zero() {
            return;
        }

        self.db
            .merged_segments
            .shift_insert(0, String::from(crate::MACOS_PAGE_ZERO_NAME), IndexSet::new());
    }
}

#[derive(Default)]
struct Symbols<'sym> {
    locals: IndexMap<(ObjectId, &'sym str), SymbolId>,
    globals: IndexMap<&'sym str, SymbolId>,
    dynamic: IndexMap<&'sym str, LibraryId>,

    referenced: IndexMap<ObjectId, Vec<&'sym str>>,
    duplicates: Vec<(SymbolId, SymbolId)>,
}

impl<'sym> Symbols<'sym> {
    fn add_local(&mut self, symbol: &'sym Symbol) {
        self.locals.insert((symbol.object, &symbol.name), symbol.id);
    }

    fn add_global(&mut self, symbol: &'sym Symbol) {
        if let Some(existing) = self.globals.insert(&symbol.name, symbol.id) {
            self.duplicates.push((existing, symbol.id));
        }
    }

    fn add_dynamic(&mut self, symbol: &'sym DynamicSymbol) {
        self.dynamic.insert(&symbol.name, symbol.library);
    }

    fn add_reference(&mut self, symbol: &'sym Symbol) {
        self.referenced.entry(symbol.object).or_default().push(&symbol.name);
    }

    fn is_symbol_defined(&self, object: ObjectId, name: &str) -> bool {
        self.locals.contains_key(&(object, name)) || self.globals.contains_key(name) || self.dynamic.contains_key(name)
    }

    /// Ensure no duplicate symbol names exist in the index.
    fn ensure_no_duplicates(&mut self, linker: &Linker) -> Result<()> {
        let mut causes = Vec::<Error>::new();

        for (existing_id, duplicate_id) in std::mem::take(&mut self.duplicates) {
            let existing_symbol = linker.db().symbol(existing_id).unwrap();
            let duplicate_symbol = linker.db().symbol(duplicate_id).unwrap();

            let existing_file = linker.db().files.get(&existing_symbol.object.file).unwrap();
            let symbol_file = linker.db().files.get(&duplicate_symbol.object.file).unwrap();

            causes.push(
                SimpleDiagnostic::new(format!("duplicate symbol {}", duplicate_symbol.name))
                    .with_help(format!("originally declared here: {}", existing_file.display()))
                    .with_help(format!("  but also declared here: {}", symbol_file.display()))
                    .into(),
            );
        }

        if !causes.is_empty() {
            return Err(SimpleDiagnostic::new("failed to build symbol index")
                .add_causes(causes)
                .into());
        }

        Ok(())
    }

    /// Ensure no unresolved symbols exist in the index.
    fn ensure_no_unresolved(&mut self, linker: &Linker) -> Result<()> {
        let mut causes = Vec::<Error>::new();

        for (&obj, symbols) in &self.referenced {
            for &symbol in symbols {
                if self.is_symbol_defined(obj, symbol) {
                    continue;
                }

                let file = linker.db().files.get(&obj.file).unwrap();

                causes.push(
                    SimpleDiagnostic::new(format!("unresolved symbol {symbol}"))
                        .with_help(format!("referenced in: {}", file.display()))
                        .into(),
                );
            }
        }

        if !causes.is_empty() {
            return Err(SimpleDiagnostic::new("unresolved symbols").add_causes(causes).into());
        }

        Ok(())
    }

    fn remove_unused_symbols(&mut self) {
        let all_referenced = self.referenced.values().flatten().copied().collect::<IndexSet<&str>>();

        self.dynamic.retain(|key, _lib| all_referenced.contains(key));
    }
}
