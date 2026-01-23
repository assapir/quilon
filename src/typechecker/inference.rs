// Type inference engine using unification

use crate::ast::Type;
use std::collections::HashMap;

/// Type variable for inference
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeVar(pub usize);

impl TypeVar {
    pub fn new(id: usize) -> Self {
        TypeVar(id)
    }
}

/// Inferred type that can contain type variables
#[derive(Debug, Clone, PartialEq)]
pub enum InferType {
    Concrete(Type),
    Variable(TypeVar),
    Function {
        params: Vec<InferType>,
        return_type: Box<InferType>,
    },
    Array(Box<InferType>),
}

/// Substitution map from type variables to types
#[derive(Debug, Clone)]
pub struct Substitution {
    map: HashMap<TypeVar, InferType>,
}

impl Substitution {
    pub fn new() -> Self {
        Substitution {
            map: HashMap::new(),
        }
    }

    pub fn empty() -> Self {
        Self::new()
    }

    pub fn bind(&mut self, var: TypeVar, ty: InferType) {
        self.map.insert(var, ty);
    }

    pub fn lookup(&self, var: &TypeVar) -> Option<&InferType> {
        self.map.get(var)
    }

    pub fn apply(&self, ty: &InferType) -> InferType {
        match ty {
            InferType::Concrete(c) => InferType::Concrete(c.clone()),
            InferType::Variable(var) => {
                if let Some(bound_ty) = self.lookup(var) {
                    self.apply(bound_ty)
                } else {
                    InferType::Variable(var.clone())
                }
            }
            InferType::Function { params, return_type } => {
                let new_params = params.iter().map(|p| self.apply(p)).collect();
                let new_return = Box::new(self.apply(return_type));
                InferType::Function {
                    params: new_params,
                    return_type: new_return,
                }
            }
            InferType::Array(elem) => {
                InferType::Array(Box::new(self.apply(elem)))
            }
        }
    }

    pub fn compose(&mut self, other: &Substitution) {
        // Apply other to all bindings in self
        for (var, ty) in &mut self.map {
            *ty = other.apply(ty);
        }

        // Add bindings from other
        for (var, ty) in &other.map {
            if !self.map.contains_key(var) {
                self.map.insert(var.clone(), ty.clone());
            }
        }
    }
}

/// Unification errors
#[derive(Debug, Clone, PartialEq)]
pub enum UnifyError {
    OccursCheck(TypeVar, InferType),
    Mismatch(InferType, InferType),
}

impl std::fmt::Display for UnifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            UnifyError::OccursCheck(var, ty) => {
                write!(f, "Occurs check failed: {:?} in {:?}", var, ty)
            }
            UnifyError::Mismatch(t1, t2) => {
                write!(f, "Type mismatch: {:?} vs {:?}", t1, t2)
            }
        }
    }
}

impl std::error::Error for UnifyError {}

/// Unification algorithm
pub fn unify(t1: &InferType, t2: &InferType) -> Result<Substitution, UnifyError> {
    match (t1, t2) {
        // Same concrete types unify
        (InferType::Concrete(c1), InferType::Concrete(c2)) if c1 == c2 => {
            Ok(Substitution::empty())
        }

        // Variable unifies with anything (if occurs check passes)
        (InferType::Variable(v), t) | (t, InferType::Variable(v)) => {
            if t == &InferType::Variable(v.clone()) {
                Ok(Substitution::empty())
            } else if occurs(v, t) {
                Err(UnifyError::OccursCheck(v.clone(), t.clone()))
            } else {
                let mut sub = Substitution::new();
                sub.bind(v.clone(), t.clone());
                Ok(sub)
            }
        }

        // Functions unify if params and return types unify
        (
            InferType::Function { params: p1, return_type: r1 },
            InferType::Function { params: p2, return_type: r2 },
        ) => {
            if p1.len() != p2.len() {
                return Err(UnifyError::Mismatch(t1.clone(), t2.clone()));
            }

            let mut sub = Substitution::empty();

            for (param1, param2) in p1.iter().zip(p2.iter()) {
                let s = unify(&sub.apply(param1), &sub.apply(param2))?;
                sub.compose(&s);
            }

            let s = unify(&sub.apply(r1), &sub.apply(r2))?;
            sub.compose(&s);

            Ok(sub)
        }

        // Arrays unify if element types unify
        (InferType::Array(e1), InferType::Array(e2)) => {
            unify(e1, e2)
        }

        // Otherwise mismatch
        _ => Err(UnifyError::Mismatch(t1.clone(), t2.clone())),
    }
}

/// Occurs check: does variable occur in type?
fn occurs(var: &TypeVar, ty: &InferType) -> bool {
    match ty {
        InferType::Concrete(_) => false,
        InferType::Variable(v) => v == var,
        InferType::Function { params, return_type } => {
            params.iter().any(|p| occurs(var, p)) || occurs(var, return_type)
        }
        InferType::Array(elem) => occurs(var, elem),
    }
}

/// Convert InferType to concrete Type (fails if type variables remain)
pub fn to_concrete(ty: &InferType) -> Option<Type> {
    match ty {
        InferType::Concrete(c) => Some(c.clone()),
        InferType::Variable(_) => None, // Can't convert unresolved variable
        InferType::Function { params, return_type } => {
            let concrete_params: Option<Vec<Type>> = params.iter().map(to_concrete).collect();
            let concrete_return = to_concrete(return_type)?;
            concrete_params.map(|p| Type::Function {
                params: p,
                return_type: Box::new(concrete_return),
            })
        }
        InferType::Array(elem) => {
            to_concrete(elem).map(|e| Type::Array(Box::new(e)))
        }
    }
}

/// Convert concrete Type to InferType
pub fn from_concrete(ty: &Type) -> InferType {
    match ty {
        Type::Num => InferType::Concrete(Type::Num),
        Type::String => InferType::Concrete(Type::String),
        Type::Bool => InferType::Concrete(Type::Bool),
        Type::Array(elem) => InferType::Array(Box::new(from_concrete(elem))),
        Type::Record(fields) => {
            InferType::Concrete(Type::Record(fields.clone()))
        }
        Type::Generic { .. } => {
            // For now, treat generics as concrete
            InferType::Concrete(ty.clone())
        }
        Type::Function { params, return_type } => InferType::Function {
            params: params.iter().map(from_concrete).collect(),
            return_type: Box::new(from_concrete(return_type)),
        },
        Type::Sum { .. } => {
            // For now, treat sum types as concrete
            InferType::Concrete(ty.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unify_concrete() {
        let t1 = InferType::Concrete(Type::Num);
        let t2 = InferType::Concrete(Type::Num);
        let result = unify(&t1, &t2);
        assert!(result.is_ok());
    }

    #[test]
    fn test_unify_variable() {
        let t1 = InferType::Variable(TypeVar::new(0));
        let t2 = InferType::Concrete(Type::Num);
        let result = unify(&t1, &t2);
        assert!(result.is_ok());
        
        let sub = result.unwrap();
        let applied = sub.apply(&t1);
        assert_eq!(applied, InferType::Concrete(Type::Num));
    }

    #[test]
    fn test_unify_mismatch() {
        let t1 = InferType::Concrete(Type::Num);
        let t2 = InferType::Concrete(Type::String);
        let result = unify(&t1, &t2);
        assert!(result.is_err());
    }

    #[test]
    fn test_unify_function() {
        let t1 = InferType::Function {
            params: vec![InferType::Concrete(Type::Num)],
            return_type: Box::new(InferType::Variable(TypeVar::new(0))),
        };
        let t2 = InferType::Function {
            params: vec![InferType::Concrete(Type::Num)],
            return_type: Box::new(InferType::Concrete(Type::String)),
        };
        
        let result = unify(&t1, &t2);
        assert!(result.is_ok());
        
        let sub = result.unwrap();
        let applied = sub.apply(&t1);
        
        if let InferType::Function { return_type, .. } = applied {
            assert_eq!(*return_type, InferType::Concrete(Type::String));
        } else {
            panic!("Expected function type");
        }
    }

    #[test]
    fn test_occurs_check() {
        let var = TypeVar::new(0);
        let t = InferType::Array(Box::new(InferType::Variable(var.clone())));
        
        let result = unify(&InferType::Variable(var), &t);
        assert!(result.is_err());
    }
}
