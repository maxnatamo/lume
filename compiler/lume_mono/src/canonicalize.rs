use indexmap::IndexMap;
use lume_architect::cached_query;
use lume_mir_queries::MirQueryCtx;
use lume_span::Internable;

use crate::*;

impl MirMonoCtx<'_, '_> {
    #[tracing::instrument(level = "DEBUG", skip_all, fields(package = self.mcx.tcx().current_package().name))]
    pub fn canonicalize(&mut self, mono_items: MonoItems) -> MonoItems {
        let functions = self
            .mcx
            .mir()
            .functions
            .values()
            .map(|func| func.id)
            .collect::<Vec<_>>();

        for func_id in functions {
            if !mono_items.any_of(func_id) {
                continue;
            }

            let mut mono_functions = IndexMap::new();

            for instance in mono_items.all_of(func_id) {
                let Some(canon_mir_func) = self.canonicalize_body(func_id, instance) else {
                    tracing::debug!("skipping canonicalization for {}", instance.display(self.mcx.tcx()));
                    continue;
                };

                mono_functions.insert(instance.to_owned(), canon_mir_func);
            }

            if !mono_functions.is_empty() {
                let base_instance = lume_mir::Instance::from(func_id);

                if tracing::enabled!(tracing::Level::DEBUG) {
                    tracing::debug!(
                        base = %base_instance.display(self.mcx.tcx()),
                        inst = %mono_functions
                            .keys()
                            .map(|inst| format!("{inst:?}").to_string())
                            .collect::<Vec<_>>()
                            .join(", "),
                        "monomorphized_function",
                    );
                }

                self.mcx.mir_mut().functions.shift_remove(&base_instance);
                self.mcx.mir_mut().functions.extend(mono_functions);
            }
        }

        let mut visitor = UpdateCallInstance {
            mcx: self.mcx,
            function_instance: None,
        };

        let mut replacement_funcs = self.mcx.mir().functions.clone();
        for func in replacement_funcs.values_mut() {
            lume_mir::walk::walk_mut(func, &mut visitor);
        }

        self.mcx.mir_mut().functions.extend(replacement_funcs);

        mono_items
    }

    #[cached_query]
    fn canonicalize_body(&self, func_id: NodeId, instance: &Instance) -> Option<lume_mir::Function> {
        // If the function has no generic components, there is nothing to monomorphize.
        let Some(generics) = &instance.generics else {
            return None;
        };

        let func_instance = lume_mir::Instance::from(func_id);
        let mut func = self.mcx.instance(&func_instance).clone();
        func.instance = instance.clone();

        let mangle_version = lume_mangle::Version::default();
        let mangle_instance = lume_mangle::Instance {
            id: func.id,
            generics: generics.iter().map(|(id, arg)| (id, arg.clone())).collect(),
        };

        func.name = instance.display(self.mcx.tcx()).to_string().intern();
        func.mangled_name = lume_mangle::mangled(self.mcx.tcx(), &mangle_instance, mangle_version)
            .unwrap_or_else(|_| panic!("bug!: could not mangle instance {}", instance.display(self.mcx.tcx())));

        Some(func)
    }
}

struct UpdateCallInstance<'mcx, 'tcx> {
    mcx: &'mcx MirQueryCtx<'tcx>,
    function_instance: Option<Instance>,
}

impl lume_mir::walk::VisitorMut for UpdateCallInstance<'_, '_> {
    fn visit_function(&mut self, func: &mut lume_mir::Function) {
        self.function_instance = Some(func.instance.clone());
    }

    fn visit_declaration(&mut self, decl: &mut lume_mir::Declaration) {
        let Some(func_instance) = self.function_instance.as_ref() else {
            return;
        };

        if let lume_mir::DeclarationKind::Call {
            instance,
            name,
            type_args,
            ..
        } = decl.kind.as_mut()
        {
            let inst_instance = self
                .mcx
                .instantiated_instance(func_instance, instance.id, std::mem::take(type_args));

            tracing::trace!(
                owner = &func_instance.display(self.mcx.tcx()).to_string(),
                before = &instance.display(self.mcx.tcx()).to_string(),
                after = &inst_instance.display(self.mcx.tcx()).to_string(),
                "update_call_site"
            );

            let new_name = inst_instance.display(self.mcx.tcx()).to_string();
            *name = new_name.intern();

            *instance = inst_instance;
        }
    }
}
