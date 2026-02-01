use std::sync::LazyLock;

use indexmap::{IndexMap, IndexSet};
use lume_errors::{DiagCtxHandle, diagnostic};
use lume_span::{Internable, Interned};

use crate::*;

pub(crate) fn index_symbols(db: &Database, dcx: &DiagCtxHandle) -> Result<SymbolDb> {
    let mut builder = SymbolDbBuilder::default();
    builder.add_symbols_from(db);

    let mut errors = Vec::new();
    let mut resolved_locals: IndexMap<LocalSymbolKey, SymbolId> = IndexMap::new();
    let mut resolved_globals: IndexMap<Interned<String>, SymbolId> = IndexMap::new();

    for (&(symbol_name, _symbol_visibility), symbol_id_set) in &builder.duplicates {
        if symbol_id_set.len() <= 1 {
            continue;
        }

        let mut symbol_iter = symbol_id_set.iter();

        let original_symbol_id = *symbol_iter.next().unwrap();
        let original_symbol_path = db.object_path(original_symbol_id.object);

        let mut diagnostic = diagnostic!("duplicate symbol {symbol_name}")
            .with_help(format!("originally declared here: {original_symbol_path}"));

        for (idx, symbol) in symbol_iter.enumerate() {
            let symbol_path = db.object_path(symbol.object);
            diagnostic = diagnostic.with_help(format!("  but also declared here: {symbol_path}"));

            if idx > 5 {
                diagnostic = diagnostic.with_help(format!("as well as {} other places", symbol_id_set.len() - idx + 1));
                break;
            }
        }

        errors.push(diagnostic);
    }

    // Resolve all referenced symbols first
    for (&bucket_key, bucket) in &builder.buckets {
        let (BucketKey::WeakReference { object } | BucketKey::StrongReference { object }) = bucket_key else {
            continue;
        };

        let is_weak_reference = matches!(bucket_key, BucketKey::WeakReference { .. });

        for referenced_name in bucket.names() {
            let Some(symbol_id) = builder.find_local_or_global(referenced_name, object) else {
                if is_weak_reference {
                    // Weak references are allowed to be unresolved
                    continue;
                }

                errors.push(
                    diagnostic!("undefined symbol {referenced_name}")
                        .with_help(format!("referenced in object: {}", db.object_path(object))),
                );

                continue;
            };

            if db.symbol(symbol_id).unwrap().linkage == Linkage::Global {
                resolved_globals.insert(referenced_name, symbol_id);
            } else {
                resolved_locals.insert(
                    LocalSymbolKey {
                        object,
                        name: referenced_name,
                    },
                    symbol_id,
                );
            }
        }
    }

    // Ensure that all global symbols are resolved as well, since they are not
    // guaranteed to be referenced (such as entrypoint symbols).
    if let Some(global_bucket) = builder.bucket(BucketKey::StrongGlobal) {
        for symbol_name in global_bucket.names() {
            let symbol_id = builder
                .find_global(symbol_name)
                .expect("globals entry must have non-empty entry");

            // TODO: should this be overwritten or kept the same?
            resolved_globals.insert(symbol_name, symbol_id);
        }
    }

    if !errors.is_empty() {
        dcx.emit_and_push(diagnostic!("failed to resolve symbols").add_causes(errors).into());
        dcx.ensure_untainted()?;
    }

    Ok(SymbolDb {
        dynamic: builder.dynamic,
        locals: resolved_locals,
        globals: resolved_globals,
    })
}

#[derive(Hash, Debug, Clone, Copy, Eq, PartialEq)]
struct LocalSymbolKey {
    pub object: ObjectId,
    pub name: Interned<String>,
}

#[derive(Hash, Debug, Clone, Copy, Eq, PartialEq)]
enum BucketKey {
    /// Bucket is limited to local symbols, which are defined within the given
    /// object.
    Local { object: ObjectId },

    /// Bucket is limited to non-weak global symbols.
    StrongGlobal,

    /// Bucket is limited to weak global symbols.
    WeakGlobal,

    /// Bucket is limited to non-weak external symbol references within the
    /// given object.
    StrongReference { object: ObjectId },

    /// Bucket is limited to weak external symbol references within the given
    /// object.
    WeakReference { object: ObjectId },
}

#[derive(Default)]
struct Bucket {
    map: IndexMap<Interned<String>, IndexSet<SymbolId>>,
    visibility: IndexMap<SymbolId, SymbolVisibility>,
}

impl Bucket {
    pub fn add_symbol(&mut self, symbol: &Symbol) {
        let name = symbol.name.base();

        self.map.entry(name).or_default().insert(symbol.id);
        self.visibility.insert(symbol.id, symbol.visibility);
    }

    pub fn names(&self) -> impl Iterator<Item = Interned<String>> {
        self.map.keys().copied()
    }

    pub fn find(&self, symbol: Interned<String>) -> Option<SymbolId> {
        static EMPTY: LazyLock<IndexSet<SymbolId>> = LazyLock::new(IndexSet::new);

        let candidates = self.map.get(&symbol).unwrap_or(&EMPTY);

        for visibility in &[
            SymbolVisibility::Default,
            SymbolVisibility::Protected,
            SymbolVisibility::Hidden,
        ] {
            if let Some(id) = candidates
                .iter()
                .find(|&id| self.visibility.get(id) == Some(visibility))
            {
                return Some(*id);
            }
        }

        None
    }
}

impl Symbol {
    fn bucket_key(&self) -> BucketKey {
        match self.linkage {
            Linkage::Local => BucketKey::Local { object: self.object },

            Linkage::External if self.weak => BucketKey::WeakReference { object: self.object },
            Linkage::External => BucketKey::StrongReference { object: self.object },

            Linkage::Global if self.weak => BucketKey::WeakGlobal,
            Linkage::Global => BucketKey::StrongGlobal,
        }
    }
}

#[derive(Default)]
struct SymbolDbBuilder {
    buckets: IndexMap<BucketKey, Bucket>,
    dynamic: IndexMap<Interned<String>, LibraryId>,

    duplicates: IndexMap<(Interned<String>, SymbolVisibility), IndexSet<SymbolId>>,
}

impl SymbolDbBuilder {
    pub fn add_symbols_from(&mut self, db: &Database) {
        for symbol in db.symbols() {
            self.add_symbol(symbol);
        }

        for framework in db.frameworks.values() {
            for symbol in &framework.symbols {
                self.add_dynamic_symbol(symbol.intern(), framework.id);
            }
        }
    }

    pub fn bucket(&self, key: BucketKey) -> Option<&Bucket> {
        self.buckets.get(&key)
    }

    pub fn ensure_bucket(&mut self, key: BucketKey) -> &mut Bucket {
        self.buckets.entry(key).or_default()
    }

    pub fn add_symbol(&mut self, symbol: &Symbol) {
        if symbol.linkage == Linkage::Global && !symbol.weak {
            let key = (symbol.name.base(), symbol.visibility);

            self.duplicates.entry(key).or_default().insert(symbol.id);
        }

        self.ensure_bucket(symbol.bucket_key()).add_symbol(symbol);
    }

    pub fn add_dynamic_symbol(&mut self, name: Interned<String>, library: LibraryId) {
        self.dynamic.insert(name, library);
    }

    pub fn find_local(&self, symbol: Interned<String>, within: ObjectId) -> Option<SymbolId> {
        self.bucket(BucketKey::Local { object: within })
            .and_then(|bucket| bucket.find(symbol))
    }

    pub fn find_global(&self, symbol: Interned<String>) -> Option<SymbolId> {
        self.bucket(BucketKey::StrongGlobal)
            .and_then(|bucket| bucket.find(symbol))
            .or_else(|| {
                self.bucket(BucketKey::WeakGlobal)
                    .and_then(|bucket| bucket.find(symbol))
            })
    }

    pub fn find_local_or_global(&self, symbol: Interned<String>, within: ObjectId) -> Option<SymbolId> {
        self.find_local(symbol, within).or_else(|| self.find_global(symbol))
    }
}

#[derive(Default)]
pub(crate) struct SymbolDb {
    locals: IndexMap<LocalSymbolKey, SymbolId>,
    globals: IndexMap<Interned<String>, SymbolId>,
    dynamic: IndexMap<Interned<String>, LibraryId>,
}

impl SymbolDb {
    //// Gets the count of all symbols in the database.
    ///
    /// Note: dynamic symbols are not included in this count.
    pub fn count(&self) -> usize {
        self.locals.len() + self.globals.len()
    }

    //// Iterates over all symbol IDs within the database.
    ///
    /// Note: since dynamic symbols aren't resolved at link time, they are not
    /// included in this iterator.
    pub fn iter_ids(&self) -> impl Iterator<Item = SymbolId> {
        self.locals.values().copied().chain(self.globals.values().copied())
    }

    //// Iterates over all symbol names within the database.
    ///
    /// Note: dynamic symbols are not included in this iterator.
    pub fn iter_names(&self) -> impl Iterator<Item = Interned<String>> {
        self.locals
            .keys()
            .map(|key| key.name)
            .chain(self.globals.keys().copied())
    }

    //// Iterates over all dynamic symbols within the database.
    pub fn dynamic(&self) -> impl Iterator<Item = (Interned<String>, LibraryId)> {
        self.dynamic
            .iter()
            .map(|(symbol_name, library_id)| (*symbol_name, *library_id))
    }

    //// Returns the symbol named `symbol`, which is defined within the object
    //// `obj`.
    #[inline]
    pub fn local_symbol(&self, symbol: Interned<String>, obj: ObjectId) -> Option<SymbolId> {
        self.locals
            .get(&LocalSymbolKey {
                object: obj,
                name: symbol,
            })
            .copied()
    }

    //// Returns the symbol named `symbol` which is defined as globals. Weak symbols
    //// are checked after non-weak symbols.
    #[inline]
    pub fn global_symbol(&self, symbol: Interned<String>) -> Option<SymbolId> {
        self.globals.get(&symbol).copied()
    }
}
