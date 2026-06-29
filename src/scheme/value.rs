#![allow(dead_code)]
/// Scheme values and lexically-scoped environments for zscheme.
/// Ported from ma-agent/src/scheme/value.rs.

use std::{cell::RefCell, collections::HashMap, fmt, rc::Rc};

use crate::scheme::parser::SchemeExpr;

// ── Environment ────────────────────────────────────────────────────────────

struct EnvInner {
    vars: HashMap<String, SchemeVal>,
    parent: Option<Env>,
}

/// A lexically-scoped environment frame.
/// `Rc<RefCell<…>>` gives cheap clone + interior mutability.
#[derive(Clone)]
pub struct Env(Rc<RefCell<EnvInner>>);

impl fmt::Debug for Env {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#<env>")
    }
}

impl Env {
    pub fn new_root() -> Self {
        Env(Rc::new(RefCell::new(EnvInner {
            vars: HashMap::new(),
            parent: None,
        })))
    }

    pub fn extend(parent: &Env) -> Self {
        Env(Rc::new(RefCell::new(EnvInner {
            vars: HashMap::new(),
            parent: Some(parent.clone()),
        })))
    }

    pub fn get(&self, name: &str) -> Option<SchemeVal> {
        let inner = self.0.borrow();
        if let Some(v) = inner.vars.get(name) {
            return Some(v.clone());
        }
        inner.parent.as_ref().and_then(|p| p.get(name))
    }

    pub fn define(&self, name: impl Into<String>, val: SchemeVal) {
        self.0.borrow_mut().vars.insert(name.into(), val);
    }

    pub fn own_bindings(&self) -> Vec<(String, SchemeVal)> {
        self.0
            .borrow()
            .vars
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn set_existing(&self, name: &str, val: SchemeVal) -> Option<()> {
        let has_locally = self.0.borrow().vars.contains_key(name);
        if has_locally {
            self.0.borrow_mut().vars.insert(name.to_string(), val);
            return Some(());
        }
        let parent = self.0.borrow().parent.clone();
        parent?.set_existing(name, val)
    }
}

// ── Value ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum SchemeVal {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Nil,
    List(Vec<SchemeVal>),
    /// A ma dot-path reference: `.my.aliases.sky`, `.my.doc.poem!publish`, etc.
    MaPath(String),
    /// A ma actor target: `@ma#house:enter`, `did:ma:abc#room:enter`, etc.
    MaActor(String),
    Builtin(String),
    Lambda {
        params: Vec<String>,
        rest: Option<String>,
        body: Vec<SchemeExpr>,
        env: Env,
    },
}

impl SchemeVal {
    pub fn display(&self) -> String {
        match self {
            SchemeVal::Str(s) => s.clone(),
            SchemeVal::Int(n) => n.to_string(),
            SchemeVal::Float(f) => f.to_string(),
            SchemeVal::Bool(true) => "#t".to_string(),
            SchemeVal::Bool(false) => "#f".to_string(),
            SchemeVal::Nil => "()".to_string(),
            SchemeVal::List(v) => {
                let inner: Vec<_> = v.iter().map(|x| x.repr()).collect();
                format!("({})", inner.join(" "))
            }
            SchemeVal::MaPath(p) => p.clone(),
            SchemeVal::MaActor(a) => a.clone(),
            SchemeVal::Builtin(n) => format!("#<procedure:{n}>"),
            SchemeVal::Lambda { .. } => "#<lambda>".to_string(),
        }
    }

    pub fn repr(&self) -> String {
        match self {
            SchemeVal::Str(s) => format!("{s:?}"),
            other => other.display(),
        }
    }

    pub fn to_splice_lossy(&self) -> String {
        match self {
            SchemeVal::Str(s) => s.clone(),
            SchemeVal::Nil => String::new(),
            SchemeVal::List(v) => v
                .iter()
                .map(|x| x.to_splice_lossy())
                .collect::<Vec<_>>()
                .join(" "),
            other => other.display(),
        }
    }

    pub fn to_splice(&self) -> Result<String, String> {
        match self {
            SchemeVal::Lambda { .. } => {
                Err("cannot splice a lambda into a command string".to_string())
            }
            SchemeVal::Builtin(n) => {
                Err(format!("cannot splice builtin '{n}' into a command string"))
            }
            other => Ok(other.to_splice_lossy()),
        }
    }

    pub fn is_truthy(&self) -> bool {
        !matches!(self, SchemeVal::Bool(false) | SchemeVal::Nil)
    }
}
