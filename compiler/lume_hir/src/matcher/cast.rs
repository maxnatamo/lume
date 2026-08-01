/// Simple type conversion of reference types, allowing for downcasting to a
/// specific type, or casting into a contained field reference.
///
/// This is used for casting [`Node`] references into their specific
/// variant types, such as [`Statement`], [`Expression`], and so
/// on. This also allows for chaining, such as casting [`Node`] into
/// [`VariableDeclaration`] ([`Node`] into [`Statement`] into
/// [`VariableDeclaration`]).
///
/// [`Node`]: crate::Node
/// [`Statement`]: crate::Statement
/// [`Expression`]: crate::Expression
/// [`VariableDeclaration`]: crate::VariableDeclaration
pub trait Cast<T> {
    /// Performs the cast.
    ///
    /// If `self` does not support casting into `T`, returns [`None`].
    fn cast(&self) -> Option<&T>;
}

macro_rules! impl_cast_node {
    ($(
        $enum_variant:ident => $target_type:ty
    ),*) => {
        $(
            impl Cast<$target_type> for crate::Node {
                fn cast(&self) -> Option<&$target_type> {
                    match self {
                        crate::Node::$enum_variant(node) => Some(node),
                        _ => None,
                    }
                }
            }
        )*
    };
}

impl_cast_node! {
    Function => crate::FunctionDefinition,
    Type => crate::TypeDefinition,
    TraitImpl => crate::TraitImplementation,
    Impl => crate::Implementation,
    Field => crate::Field,
    Parameter => crate::Parameter,
    Method => crate::MethodDefinition,
    TraitMethodDef => crate::TraitMethodDefinition,
    TraitMethodImpl => crate::TraitMethodImplementation,
    Pattern => crate::Pattern,
    Statement => crate::Statement,
    Expression => crate::Expression
}

impl Cast<crate::EnumDefinition> for crate::Node {
    fn cast(&self) -> Option<&crate::EnumDefinition> {
        let def: &crate::TypeDefinition = self.cast()?;

        match def {
            crate::TypeDefinition::Enum(node) => Some(node),
            _ => None,
        }
    }
}

impl Cast<crate::StructDefinition> for crate::Node {
    fn cast(&self) -> Option<&crate::StructDefinition> {
        let def: &crate::TypeDefinition = self.cast()?;

        match def {
            crate::TypeDefinition::Struct(node) => Some(node),
            _ => None,
        }
    }
}

impl Cast<crate::TraitDefinition> for crate::Node {
    fn cast(&self) -> Option<&crate::TraitDefinition> {
        let def: &crate::TypeDefinition = self.cast()?;

        match def {
            crate::TypeDefinition::Trait(node) => Some(node),
            _ => None,
        }
    }
}

impl Cast<crate::TypeParameter> for crate::Node {
    fn cast(&self) -> Option<&crate::TypeParameter> {
        let def: &crate::TypeDefinition = self.cast()?;

        match def {
            crate::TypeDefinition::TypeParameter(node) => Some(node),
            _ => None,
        }
    }
}

macro_rules! impl_try_from_stmt {
    ($(
        $enum_variant:ident => $target_type:ty
    ),*) => {
        $(
            impl Cast<$target_type> for crate::Node {
                fn cast(&self) -> Option<&$target_type> {
                    let stmt: &crate::Statement = self.cast()?;

                    match &stmt.kind {
                        crate::StatementKind::$enum_variant(node) => Some(node),
                        _ => None,
                    }
                }
            }
        )*
    };
}

impl_try_from_stmt! {
    Variable => crate::VariableDeclaration,
    Break => crate::Break,
    Continue => crate::Continue,
    Final => crate::Final,
    Return => crate::Return,
    InfiniteLoop => crate::InfiniteLoop
}

macro_rules! impl_try_from_expr {
    ($(
        $enum_variant:ident => $target_type:ty
    ),*) => {
        $(
            impl Cast<$target_type> for crate::Node {
                fn cast(&self) -> Option<&$target_type> {
                    let expr: &crate::Expression = self.cast()?;

                    match &expr.kind {
                        crate::ExpressionKind::$enum_variant(node) => Some(node),
                        _ => None,
                    }
                }
            }
        )*
    };
}

impl_try_from_expr! {
    Assignment => crate::Assignment,
    Cast => crate::Cast,
    Construct => crate::Construct,
    Deref => crate::DerefExpr,
    Ref => crate::RefExpr,
    StaticCall => crate::StaticCall,
    InstanceCall => crate::InstanceCall,
    IntrinsicCall => crate::IntrinsicCall,
    If => crate::If,
    Is => crate::Is,
    Literal => crate::Literal,
    Member => crate::Member,
    Scope => crate::Scope,
    Switch => crate::Switch,
    Variable => crate::Variable,
    Variant => crate::Variant
}

macro_rules! impl_try_from_pattern {
    ($(
        $enum_variant:ident => $target_type:ty
    ),*) => {
        $(
            impl Cast<$target_type> for crate::Node {
                fn cast(&self) -> Option<&$target_type> {
                    let pat: &crate::Pattern = self.cast()?;

                    match &pat.kind {
                        crate::PatternKind::$enum_variant(node) => Some(node),
                        _ => None,
                    }
                }
            }
        )*
    };
}

impl_try_from_pattern! {
    Literal => crate::LiteralPattern,
    Identifier => crate::IdentifierPattern,
    Variant => crate::VariantPattern,
    Wildcard => crate::WildcardPattern
}
