//! The lexical environment: a stack of scopes mapping a name to what it was bound to.

use super::*;

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl Environment {
    pub fn new() -> Self {
        Environment {
            scopes: vec![HashMap::new()],
            enforce_reserved_names: true,
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// Define a binding whose value is fresh — reachable through no other binding. Every
    /// binding starts this way; the aliasing-aware definers below say more.
    pub fn define(
        &mut self,
        name: String,
        type_: Type,
        mutable: bool,
        span: Span,
    ) -> Result<(), TypeError> {
        self.define_symbol(
            name,
            Symbol {
                type_,
                mutable,
                owner: 0,
                value_aliasing: ValueAliasing::default(),
                result_aliasing: None,
                setter_receiver: false,
                constant: false,
            },
            span,
        )
    }

    /// Define a payload-less constant value (a nullary sum variant): shared, but with no
    /// writable interior, so aliasing it is unrestricted.
    pub(super) fn define_constant(
        &mut self,
        name: String,
        type_: Type,
        span: Span,
    ) -> Result<(), TypeError> {
        self.define_symbol(
            name,
            Symbol {
                type_,
                mutable: false,
                owner: 0,
                value_aliasing: ValueAliasing::default(),
                result_aliasing: None,
                setter_receiver: false,
                constant: true,
            },
            span,
        )
    }

    /// Define a parameter: its value belongs to the caller, recorded as its own argument
    /// slot under `declaration`.
    pub(super) fn define_parameter(
        &mut self,
        name: String,
        type_: Type,
        declaration: u64,
        slot: usize,
        span: Span,
    ) -> Result<(), TypeError> {
        let symbol = Self::parameter_symbol(&name, type_, declaration, slot);
        self.define_symbol(name, symbol, span)
    }

    /// Define an `=` method's receiver `it` — parameter slot 0. The checker's own binding
    /// of a reserved name, so it skips the reserved-name check a user's parameter gets.
    pub(super) fn define_receiver(
        &mut self,
        type_: Type,
        declaration: u64,
        span: Span,
    ) -> Result<(), TypeError> {
        let symbol = Self::parameter_symbol(crate::ast::RECEIVER, type_, declaration, 0);
        self.insert_symbol(crate::ast::RECEIVER.to_string(), symbol, span)
    }

    fn parameter_symbol(name: &str, type_: Type, declaration: u64, slot: usize) -> Symbol {
        Symbol {
            type_,
            mutable: false,
            owner: declaration,
            value_aliasing: ValueAliasing {
                parameters: vec![(declaration, slot, name.to_string())],
                ..ValueAliasing::default()
            },
            result_aliasing: None,
            setter_receiver: false,
            constant: false,
        }
    }

    /// Register one of the checker's own built-in types (`Result`, `Site`) under its
    /// reserved name — the one registration the reserved-name check does not apply to.
    pub(super) fn define_builtin(&mut self, name: &str, type_: Type) {
        let symbol = Symbol {
            type_,
            mutable: false,
            owner: 0,
            value_aliasing: ValueAliasing::default(),
            result_aliasing: None,
            setter_receiver: false,
            constant: false,
        };
        let root = self.scopes.first_mut().expect("the root scope exists");
        root.insert(name.to_string(), symbol);
    }

    /// Define a setter's receiver `it`: immutable as a NAME (no rebinding), but its value
    /// is mutable at every call site. `owner` is the ENCLOSING declaration, so the
    /// receiver — which outlives the setter's own frame — survives the return filter.
    /// The checker's own binding of a reserved name, so no reserved-name check.
    pub(super) fn define_setter_receiver(
        &mut self,
        name: String,
        type_: Type,
        owner: u64,
        span: Span,
    ) -> Result<(), TypeError> {
        self.insert_symbol(
            name,
            Symbol {
                type_,
                mutable: false,
                owner,
                value_aliasing: ValueAliasing::default(),
                result_aliasing: None,
                setter_receiver: true,
                constant: false,
            },
            span,
        )
    }

    /// Define a binding whose value may alias other bindings (its initializer's or the
    /// matched scrutinee's aliasing), owned by `owner`.
    pub(super) fn define_binding(
        &mut self,
        name: String,
        type_: Type,
        mutable: bool,
        owner: u64,
        value_aliasing: ValueAliasing,
        span: Span,
    ) -> Result<(), TypeError> {
        self.define_symbol(
            name,
            Symbol {
                type_,
                mutable,
                owner,
                value_aliasing,
                result_aliasing: None,
                setter_receiver: false,
                constant: false,
            },
            span,
        )
    }

    /// Bind a user-written name: refused when the language reserves it (see
    /// `ast::reserved_for`), then when the current scope already holds it.
    fn define_symbol(&mut self, name: String, symbol: Symbol, span: Span) -> Result<(), TypeError> {
        if self.enforce_reserved_names
            && let Some(error) = TypeError::reserved_name(&name, &span)
        {
            return Err(error);
        }
        self.insert_symbol(name, symbol, span)
    }

    fn insert_symbol(&mut self, name: String, symbol: Symbol, span: Span) -> Result<(), TypeError> {
        let current_scope = self.scopes.last_mut().unwrap();

        if current_scope.contains_key(&name) {
            return Err(TypeError::DuplicateDefinition { name, span });
        }

        current_scope.insert(name, symbol);
        Ok(())
    }

    pub fn lookup(&self, name: &str) -> Option<&Symbol> {
        for scope in self.scopes.iter().rev() {
            if let Some(symbol) = scope.get(name) {
                return Some(symbol);
            }
        }
        None
    }

    fn lookup_mut(&mut self, name: &str) -> Option<&mut Symbol> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(symbol) = scope.get_mut(name) {
                return Some(symbol);
            }
        }
        None
    }

    pub fn get_type(&self, name: &str) -> Option<Type> {
        self.lookup(name).map(|s| s.type_.clone())
    }

    pub fn is_mutable(&self, name: &str) -> bool {
        self.lookup(name).map(|s| s.mutable).unwrap_or(false)
    }

    /// Update a binding's type (used for function type inference).
    /// Returns `true` if a binding was found and updated.
    pub fn update_type(&mut self, name: &str, new_type: Type) -> bool {
        match self.lookup_mut(name) {
            Some(symbol) => {
                symbol.type_ = new_type;
                true
            }
            None => false,
        }
    }

    /// Record a named function's classified result aliasing on its binding, once its
    /// body has been checked.
    pub(super) fn set_result_aliasing(&mut self, name: &str, result_aliasing: ResultAliasing) {
        if let Some(symbol) = self.lookup_mut(name) {
            symbol.result_aliasing = Some(result_aliasing);
        }
    }
}
