#![allow(dead_code)]
/// Scheme module: session environment + public evaluation API.

pub mod eval;
pub mod parser;
pub mod value;

pub use eval::{eval, SchemeErr};
pub use value::{Env, SchemeVal};

#[allow(unused_imports)]
pub use eval::eval_str;

use std::cell::RefCell;

use crate::context::Ctx;
use parser::{parse_expr, tokenize};

// ── Session environment ────────────────────────────────────────────────────

thread_local! {
    static SESSION_ENV: RefCell<Option<Env>> = const { RefCell::new(None) };
}

/// Initialise a fresh session environment.
pub fn init_session_env() {
    SESSION_ENV.with(|e| *e.borrow_mut() = Some(Env::new_root()));
}

/// Clear the session environment.
pub fn reset_session_env() {
    SESSION_ENV.with(|e| *e.borrow_mut() = None);
}

/// Return the current session environment, creating one if needed.
pub(crate) fn get_env() -> Env {
    SESSION_ENV.with(|e| {
        let mut inner = e.borrow_mut();
        if inner.is_none() {
            *inner = Some(Env::new_root());
        }
        inner.as_ref().unwrap().clone()
    })
}

// ── Public evaluation API ──────────────────────────────────────────────────

/// Evaluate all top-level expressions in `source` in the session environment.
/// Returns the value of the last expression.
pub async fn eval_source(source: &str, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    let env = get_env();
    let tokens = tokenize(source).map_err(|e| SchemeErr::ParseError(e.to_string()))?;
    let mut pos = 0;
    let mut last = SchemeVal::Nil;
    while pos < tokens.len() {
        let (expr, next) =
            parse_expr(&tokens, pos).map_err(|e| SchemeErr::ParseError(e.to_string()))?;
        last = eval(expr, env.clone(), ctx.clone()).await?;
        pos = next;
    }
    Ok(last)
}
