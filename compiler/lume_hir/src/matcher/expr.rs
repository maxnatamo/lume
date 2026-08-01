use crate::matcher::{Bindings, Predicate, PredicateBox};
use crate::*;

lumec_matchers! {
    /// Matches all expressions.
    matcher expression(predicates) -> bool {
        type Parent = Node;
        type Subject = Expression;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let Node::Expression(subject) = subject {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all assignment expressions (`as`).
    matcher assignment(predicates) -> bool {
        type Parent = Expression;
        type Subject = Assignment;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::Assignment(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all cast expressions (`as`).
    matcher cast(predicates) -> bool {
        type Parent = Expression;
        type Subject = Cast;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::Cast(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all construct expressions.
    matcher construct(predicates) -> bool {
        type Parent = Expression;
        type Subject = Construct;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::Construct(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all dereference expressions (`*`).
    matcher deref_expr(predicates) -> bool {
        type Parent = Expression;
        type Subject = DerefExpr;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::Deref(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all reference expressions (`&`).
    matcher ref_expr(predicates) -> bool {
        type Parent = Expression;
        type Subject = RefExpr;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::Ref(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all intrinsic call expressions.
    matcher intrinsic_call(predicates) -> bool {
        type Parent = Expression;
        type Subject = IntrinsicCall;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::IntrinsicCall(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all instance call expressions.
    matcher instance_call(predicates) -> bool {
        type Parent = Expression;
        type Subject = InstanceCall;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::InstanceCall(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all static call expressions.
    matcher static_call(predicates) -> bool {
        type Parent = Expression;
        type Subject = StaticCall;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::StaticCall(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all `if` expressions.
    matcher if_expr(predicates) -> bool {
        type Parent = Expression;
        type Subject = If;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::If(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all `is` expressions.
    matcher is(predicates) -> bool {
        type Parent = Expression;
        type Subject = Is;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::Is(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all literal expressions.
    matcher literal(predicates) -> bool {
        type Parent = Expression;
        type Subject = Literal;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::Literal(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all member expressions.
    matcher member(predicates) -> bool {
        type Parent = Expression;
        type Subject = Member;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::Member(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all scope expressions.
    matcher scope(predicates) -> bool {
        type Parent = Expression;
        type Subject = Scope;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::Scope(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all variable references.
    matcher var_reference(predicates) -> bool {
        type Parent = Expression;
        type Subject = Variable;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::Variable(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches all variant expressions.
    matcher variant(predicates) -> bool {
        type Parent = Expression;
        type Subject = Variant;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::Variant(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }
}

lumec_matchers! {
    /// Matches all switch expressions.
    matcher switch(predicates) -> bool {
        type Parent = Expression;
        type Subject = Switch;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            if let ExpressionKind::Switch(subject) = &subject.kind {
                Predicate::satisfies(&self.predicates, hir, subject, bindings)
            } else {
                false
            }
        }
    }

    /// Matches the operand of a switch expression.
    matcher has_operand(predicates) -> bool {
        type Parent = Switch;
        type Subject = Expression;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            let operand = hir.expect_expression(subject.operand).unwrap();

            Predicate::satisfies(&self.predicates, hir, operand, bindings)
        }
    }

    /// Matches a switch arm of a switch expression.
    matcher has_case(predicates) -> bool {
        type Parent = Switch;
        type Subject = SwitchCase;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            subject.cases.iter().any(|case| Predicate::satisfies(&self.predicates, hir, case, bindings))
        }
    }

    /// Matches the branch of a switch case arm.
    matcher has_branch(predicates) -> bool {
        type Parent = SwitchCase;
        type Subject = Expression;

        fn satisfies(&self, hir: &Map, subject: &Self::Subject, bindings: &mut Bindings) -> bool {
            let operand = hir.expect_expression(subject.branch).unwrap();

            Predicate::satisfies(&self.predicates, hir, operand, bindings)
        }
    }
}
