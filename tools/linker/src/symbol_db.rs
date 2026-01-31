use std::sync::atomic::AtomicBool;

use indexmap::{IndexMap, IndexSet};
use lume_errors::{DiagCtxHandle, diagnostic};
use lume_span::{Internable, Interned};

use crate::*;

pub(crate) fn create_symbol_db(db: &Database, dcx: &DiagCtxHandle) -> Result<SymbolDb> {
    let mut builder = SymbolDbBuilder::new();

    for symbol in db.symbols() {
        builder.add(symbol);
    }

    for framework in db.frameworks.values() {
        for symbol in &framework.symbols {
            builder.add_dynamic(symbol.intern(), framework.id);
        }
    }

    builder.finalize(db, dcx)
}

#[derive(Hash, Debug, Clone, Copy, Eq, PartialEq)]
struct LocalSymbolKey {
    pub object: ObjectId,
    pub name: Interned<String>,
}

#[derive(Default)]
pub(crate) struct SymbolDbBuilder {
    references: IndexMap<ObjectId, IndexSet<Interned<String>>>,

    locals: IndexMap<LocalSymbolKey, IndexSet<SymbolId>>,
    globals: IndexMap<Interned<String>, IndexSet<SymbolId>>,
    weak_globals: IndexMap<Interned<String>, IndexSet<SymbolId>>,
    dynamic: IndexMap<Interned<String>, LibraryId>,

    finalized: AtomicBool,
}

impl SymbolDbBuilder {
    pub fn new() -> Self {
        SymbolDbBuilder::default()
    }

    /// Adds the given symbol to the database.
    pub fn add(&mut self, symbol: &Symbol) {
        let object = symbol.object;
        let name = symbol.name.base();

        match symbol.linkage {
            Linkage::Local => {
                let key = LocalSymbolKey { object, name };
                self.locals.entry(key).or_default().insert(symbol.id);
            }
            Linkage::Global { weak } => {
                if weak {
                    self.weak_globals.entry(name).or_default().insert(symbol.id);
                } else {
                    self.globals.entry(name).or_default().insert(symbol.id);
                }
            }
            Linkage::External => {
                self.references.entry(object).or_default().insert(name);
            }
        }
    }

    /// Adds the given dynamic symbol to the database.
    pub fn add_dynamic(&mut self, symbol: Interned<String>, library: LibraryId) {
        self.dynamic.insert(symbol, library);
    }
}

impl SymbolDbBuilder {
    /// Marks the symbol database as finalized and returns the finalized symbol
    /// database.
    ///
    /// After the database is finalized, no more symbols can be added to it.
    ///
    /// # Panics
    ///
    /// Panics if the symbol database is already finalized.
    ///
    /// # Errors
    ///
    /// Returns [`Err`] if multiple non-weak global symbols are defined with the
    /// same name or if a referenced symbol is left undefined.
    pub fn finalize(mut self, db: &Database, dcx: &DiagCtxHandle) -> Result<SymbolDb> {
        assert!(
            !self.finalized.swap(true, std::sync::atomic::Ordering::Relaxed),
            "bug!: symbol database is already finalized"
        );

        self.assert_all_defined(db, dcx);
        self.assert_no_duplicates(db, dcx);
        self.remove_unused_symbols();

        self.finalized.store(true, std::sync::atomic::Ordering::Relaxed);
        dcx.ensure_untainted()?;

        let mut symdb = SymbolDb {
            dynamic: self.dynamic,
            ..SymbolDb::default()
        };

        for (symbol_key, symbols) in self.locals {
            let symbol_id = *symbols.first().expect("local symbol must be resolved");
            symdb.locals.insert(symbol_key, symbol_id);
        }

        for (symbol_name, symbols) in self.globals {
            let symbol_id = *symbols.first().expect("global symbol must be resolved");
            symdb.globals.insert(symbol_name, symbol_id);
        }

        for (symbol_name, symbols) in self.weak_globals {
            let symbol_id = *symbols.first().expect("weak global symbol must be resolved");
            symdb.weak_globals.insert(symbol_name, symbol_id);
        }

        Ok(symdb)
    }

    /// Asserts that all referenced symbols are defined.
    fn assert_all_defined(&self, db: &Database, dcx: &DiagCtxHandle) {
        for (object_id, references) in &self.references {
            let object_path = db.object_path(*object_id);

            for reference in references {
                let mut matches = self.symbol_within_or_global(*reference, *object_id);

                if matches.next().is_none() {
                    dcx.emit_and_push(
                        diagnostic!("undefined symbol {reference}")
                            .with_help(format!("referenced in object: {object_path}"))
                            .into(),
                    );
                }
            }
        }
    }

    /// Asserts that no duplicate symbols are defined.
    fn assert_no_duplicates(&self, db: &Database, dcx: &DiagCtxHandle) {
        fn assert_within<'set, I>(iter: I, db: &Database, dcx: &DiagCtxHandle)
        where
            I: Iterator<Item = (Interned<String>, &'set IndexSet<SymbolId>)>,
        {
            for (name, symbols) in iter {
                if symbols.len() <= 1 {
                    continue;
                }

                let mut symbol_iter = symbols.iter();

                let original_symbol_id = *symbol_iter.next().unwrap();
                let original_symbol_path = db.object_path(original_symbol_id.object);

                let mut diagnostic = diagnostic!("duplicate symbol {name}")
                    .with_help(format!("originally declared here: {original_symbol_path}"));

                for (idx, symbol) in symbol_iter.enumerate() {
                    let symbol_path = db.object_path(symbol.object);
                    diagnostic = diagnostic.with_help(format!("  but also declared here: {symbol_path}"));

                    if idx > 5 {
                        diagnostic =
                            diagnostic.with_help(format!("as well as {} other places", symbols.len() - idx + 1));
                        break;
                    }
                }

                dcx.emit_and_push(diagnostic.into());
            }
        }

        assert_within(self.globals.iter().map(|(name, refs)| (*name, refs)), db, dcx);
        assert_within(self.locals.iter().map(|(name, refs)| (name.name, refs)), db, dcx);
    }

    /// Removes all symbols from the database which aren't referenced.
    fn remove_unused_symbols(&mut self) {
        let all_referenced = self
            .references
            .values()
            .flatten()
            .copied()
            .collect::<IndexSet<Interned<String>>>();

        self.dynamic.retain(|key, _lib| all_referenced.contains(key));
        self.globals.retain(|key, _syms| all_referenced.contains(key));
        self.weak_globals
            .retain(|key, _syms| self.globals.contains_key(key) || all_referenced.contains(key));

        self.globals.values_mut().for_each(|syms| {
            syms.drain(1..);
        });

        self.weak_globals.values_mut().for_each(|syms| {
            syms.drain(1..);
        });
    }

    fn ensure_finalized(&self) {
        assert!(
            self.finalized.load(std::sync::atomic::Ordering::Relaxed),
            "bug!: symbol database has not been finalized"
        );
    }
}

impl SymbolDbBuilder {
    //// Returns an iterator of all symbols named `symbol`, which are defined within
    //// the object `obj`.
    pub fn symbol_within(&self, symbol: Interned<String>, obj: ObjectId) -> impl Iterator<Item = SymbolId> {
        static EMPTY: &indexmap::set::Slice<SymbolId> = indexmap::set::Slice::<SymbolId>::new();

        self.ensure_finalized();

        self.locals
            .get(&LocalSymbolKey {
                object: obj,
                name: symbol,
            })
            .map_or(EMPTY, |set| set.as_slice())
            .iter()
            .copied()
    }

    //// Returns an iterator of all symbols named `symbol` which are defined as
    //// globals. Weak symbols are iterated after non-weak symbols.
    pub fn global_symbols(&self, symbol: Interned<String>) -> impl Iterator<Item = SymbolId> {
        static EMPTY: &indexmap::set::Slice<SymbolId> = indexmap::set::Slice::<SymbolId>::new();

        self.ensure_finalized();

        let strong = self
            .globals
            .get(&symbol)
            .map_or(EMPTY, |set| set.as_slice())
            .iter()
            .copied();

        let weak = self
            .weak_globals
            .get(&symbol)
            .map_or(EMPTY, |set| set.as_slice())
            .iter()
            .copied();

        strong.chain(weak)
    }

    //// Returns an iterator of all symbols named `symbol`, which are defined within
    //// the object `obj`.
    ///
    /// If the local symbols are exhausted, the iterator will yield weak global
    /// symbols, then non-weak global symbols.
    pub fn symbol_within_or_global(&self, symbol: Interned<String>, obj: ObjectId) -> impl Iterator<Item = SymbolId> {
        self.ensure_finalized();
        self.symbol_within(symbol, obj).chain(self.global_symbols(symbol))
    }
}

#[derive(Default)]
pub(crate) struct SymbolDb {
    locals: IndexMap<LocalSymbolKey, SymbolId>,
    globals: IndexMap<Interned<String>, SymbolId>,
    weak_globals: IndexMap<Interned<String>, SymbolId>,
    dynamic: IndexMap<Interned<String>, LibraryId>,
}

impl SymbolDb {
    //// Gets the count of all symbols in the database.
    pub fn count(&self) -> usize {
        self.locals.len() + self.globals.len() + self.weak_globals.len() + self.dynamic.len()
    }

    //// Iterates over all symbol IDs within the database.
    pub fn iter_ids(&self) -> impl Iterator<Item = SymbolId> {
        self.locals
            .values()
            .copied()
            .chain(self.globals.values().copied())
            .chain(self.weak_globals.values().copied())
    }

    //// Iterates over all symbol names within the database.
    pub fn iter_names(&self) -> impl Iterator<Item = Interned<String>> {
        self.locals
            .keys()
            .map(|key| key.name)
            .chain(self.globals.keys().copied())
            .chain(self.weak_globals.keys().copied())
            .chain(self.dynamic.keys().copied())
    }

    //// Iterates over all dynamic symbols within the database.
    pub fn dynamic(&self) -> impl Iterator<Item = (Interned<String>, LibraryId)> {
        self.dynamic
            .iter()
            .map(|(symbol_name, library_id)| (*symbol_name, *library_id))
    }

    //// Returns the symbol named `symbol`, which is defined within the object
    //// `obj`.
    pub fn symbol_within(&self, symbol: Interned<String>, obj: ObjectId) -> Option<SymbolId> {
        self.locals
            .get(&LocalSymbolKey {
                object: obj,
                name: symbol,
            })
            .copied()
    }

    //// Returns the symbol named `symbol` which is defined as globals. Weak symbols
    //// are checked after non-weak symbols.
    pub fn global_symbol(&self, symbol: Interned<String>) -> Option<SymbolId> {
        if let Some(symbol_id) = self.globals.get(&symbol).copied() {
            return Some(symbol_id);
        };

        if let Some(symbol_id) = self.weak_globals.get(&symbol).copied() {
            return Some(symbol_id);
        };

        None
    }

    //// Returns the symbol named `symbol`, which is defined within the object
    //// `obj`.
    ///
    /// If no matching local symbol is found, non-weak global symbols are
    /// queried, then weak global symbols.
    pub fn symbol_within_or_global(&self, symbol: Interned<String>, obj: ObjectId) -> Option<SymbolId> {
        self.symbol_within(symbol, obj).or_else(|| self.global_symbol(symbol))
    }
}
