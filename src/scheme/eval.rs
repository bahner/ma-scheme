/// Async Scheme evaluator for zscheme.
///
/// Ported from ma-agent/src/scheme/eval.rs with the following changes:
/// - `Rc<EvalCtx { AppState, RwSignal<EgoConfig> }>` → `Rc<CliCtx>`
/// - `LocalBoxFuture` kept (runs inside a tokio LocalSet)
/// - `spawn_local` → `tokio::task::spawn_local`
/// - `ctx.state.push_system(text)` → `ctx.display_output(text)`
/// - All CID fetch, RPC send, config access go through `ctx` methods

use std::rc::Rc;

use futures::future::LocalBoxFuture;

use crate::context::CliCtx;
use crate::scheme::{
    parser::{parse_expr, tokenize, SchemeExpr},
    value::{Env, SchemeVal},
};

use crate::config::is_link_value;

// ── Context type alias ─────────────────────────────────────────────────────

pub type Ctx = Rc<CliCtx>;

// ── Error ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SchemeErr {
    Arity {
        name: String,
        expected: usize,
        got: usize,
    },
    Runtime(String),
    Undefined(String),
    #[allow(dead_code)]
    ParseError(String),
    MaError(String),
}

impl std::fmt::Display for SchemeErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemeErr::Arity { name, expected, got } => {
                write!(f, "{name}: expected {expected} args, got {got}")
            }
            SchemeErr::Runtime(s) => write!(f, "{s}"),
            SchemeErr::Undefined(s) => write!(f, "undefined: {s}"),
            SchemeErr::ParseError(s) => write!(f, "parse error: {s}"),
            SchemeErr::MaError(s) => write!(f, "ma: {s}"),
        }
    }
}

// ── Entry points ───────────────────────────────────────────────────────────

/// Evaluate a Scheme source string.
pub fn eval_str(
    source: &str,
    env: Env,
    ctx: Ctx,
) -> LocalBoxFuture<'static, Result<SchemeVal, SchemeErr>> {
    let source = source.to_string();
    Box::pin(async move {
        let tokens = tokenize(&source).map_err(|e| SchemeErr::ParseError(e.to_string()))?;
        let (expr, _) = parse_expr(&tokens, 0).map_err(|e| SchemeErr::ParseError(e.to_string()))?;
        eval(expr, env, ctx).await
    })
}

/// Evaluate a parsed `SchemeExpr`.
pub fn eval(
    expr: SchemeExpr,
    env: Env,
    ctx: Ctx,
) -> LocalBoxFuture<'static, Result<SchemeVal, SchemeErr>> {
    Box::pin(async move { eval_inner(expr, env, ctx).await })
}

// ── Core evaluator ─────────────────────────────────────────────────────────

async fn eval_inner(expr: SchemeExpr, env: Env, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    match expr {
        SchemeExpr::Nil => Ok(SchemeVal::Nil),
        SchemeExpr::Str(s) => Ok(SchemeVal::Str(s)),
        SchemeExpr::Atom(s) => {
            // CID literal: <bafy…> — fetch from IPFS, return content as string.
            if s.starts_with('<') && s.ends_with('>') && s.len() > 2 {
                let inner = s[1..s.len() - 1].to_string();
                return if is_link_value(&inner) {
                    ctx.fetch_cid(&inner)
                        .await
                        .map(SchemeVal::Str)
                        .map_err(SchemeErr::MaError)
                } else {
                    Err(SchemeErr::Runtime(format!("not a valid CID: {inner}")))
                };
            }
            // ma dot-path in value position.
            if s.starts_with('.') {
                return ctx.eval_dot(&s).map_err(|e| e);
            }
            eval_atom(&s, &env)
        }
        SchemeExpr::List(forms) => {
            if forms.is_empty() {
                return Ok(SchemeVal::Nil);
            }

            // ── Special forms ──────────────────────────────────────────────
            if let SchemeExpr::Atom(head) = &forms[0] {
                match head.as_str() {
                    "define" => return eval_define(forms, env, ctx).await,
                    "lambda" | "ʎ" => return eval_lambda(forms, env),
                    "let" => return eval_let(forms, env, ctx, false).await,
                    "let*" => return eval_let(forms, env, ctx, true).await,
                    "letrec" => return eval_letrec(forms, env, ctx).await,
                    "if" => return eval_if(forms, env, ctx).await,
                    "cond" => return eval_cond(forms, env, ctx).await,
                    "begin" => return eval_begin(&forms[1..], env, ctx).await,
                    "quote" => {
                        if forms.len() != 2 {
                            return Err(SchemeErr::Runtime(
                                "quote: expected exactly one argument".to_string(),
                            ));
                        }
                        return Ok(expr_to_val(&forms[1]));
                    }
                    "and" => return eval_and(&forms[1..], env, ctx).await,
                    "or" => return eval_or(&forms[1..], env, ctx).await,
                    "when" => {
                        if forms.len() < 3 {
                            return Err(SchemeErr::Runtime(
                                "when: expected condition + body".to_string(),
                            ));
                        }
                        let cond = eval(forms[1].clone(), env.clone(), ctx.clone()).await?;
                        return if cond.is_truthy() {
                            eval_begin(&forms[2..], env, ctx).await
                        } else {
                            Ok(SchemeVal::Nil)
                        };
                    }
                    "unless" => {
                        if forms.len() < 3 {
                            return Err(SchemeErr::Runtime(
                                "unless: expected condition + body".to_string(),
                            ));
                        }
                        let cond = eval(forms[1].clone(), env.clone(), ctx.clone()).await?;
                        return if !cond.is_truthy() {
                            eval_begin(&forms[2..], env, ctx).await
                        } else {
                            Ok(SchemeVal::Nil)
                        };
                    }
                    "set!" => {
                        if forms.len() != 3 {
                            return Err(SchemeErr::Runtime(
                                "set!: expected symbol and value".to_string(),
                            ));
                        }
                        let name = atom_name(&forms[1], "set!")?;
                        let val = eval(forms[2].clone(), env.clone(), ctx).await?;
                        env.set_existing(&name, val)
                            .ok_or(SchemeErr::Undefined(name))?;
                        return Ok(SchemeVal::Nil);
                    }
                    _ => {}
                }
            }

            // ── Pipe threading: (val | (f arg) | g) ───────────────────────
            if forms
                .iter()
                .skip(1)
                .any(|f| matches!(f, SchemeExpr::Atom(s) if s == "|"))
            {
                return eval_pipe(forms, env, ctx).await;
            }

            // ── ma dot-path in head position ────────────────────────────────
            if let SchemeExpr::Atom(head) = &forms[0] {
                if head.starts_with('.') {
                    if forms.len() == 1 {
                        let val = ctx.eval_dot(head)?;
                        if let SchemeVal::Str(ref s) = val {
                            if s.trim_start().starts_with('(') {
                                let tokens = tokenize(s)
                                    .map_err(|e| SchemeErr::ParseError(e.to_string()))?;
                                let (expr, _) = parse_expr(&tokens, 0)
                                    .map_err(|e| SchemeErr::ParseError(e.to_string()))?;
                                return eval(expr, env.clone(), ctx.clone()).await;
                            }
                        }
                        return Ok(val);
                    }
                    let path = SchemeVal::MaPath(head.clone());
                    let mut args = Vec::with_capacity(forms.len() - 1);
                    for form in &forms[1..] {
                        args.push(eval(form.clone(), env.clone(), ctx.clone()).await?);
                    }
                    return apply(path, args, ctx).await;
                }
            }

            // ── Application ────────────────────────────────────────────────
            let head_val = eval(forms[0].clone(), env.clone(), ctx.clone()).await?;

            let mut args = Vec::with_capacity(forms.len() - 1);
            for form in &forms[1..] {
                args.push(eval(form.clone(), env.clone(), ctx.clone()).await?);
            }

            apply(head_val, args, ctx).await
        }
    }
}

// ── Atom evaluation ────────────────────────────────────────────────────────

fn eval_atom(s: &str, env: &Env) -> Result<SchemeVal, SchemeErr> {
    if let Some(v) = env.get(s) {
        return Ok(v);
    }
    if let Ok(n) = s.parse::<i64>() {
        return Ok(SchemeVal::Int(n));
    }
    if let Ok(f) = s.parse::<f64>() {
        return Ok(SchemeVal::Float(f));
    }
    if s == "#t" || s == "true" {
        return Ok(SchemeVal::Bool(true));
    }
    if s == "#f" || s == "false" {
        return Ok(SchemeVal::Bool(false));
    }
    if s == "nil" || s == "()" {
        return Ok(SchemeVal::Nil);
    }
    // ma fragment atoms like `#room`, `#house:enter` — treat as strings.
    if s.starts_with('#') {
        return Ok(SchemeVal::Str(s.to_string()));
    }
    if s.starts_with('.') {
        return Ok(SchemeVal::MaPath(s.to_string()));
    }
    if s.starts_with('@') {
        return Ok(SchemeVal::MaActor(s.to_string()));
    }
    if is_builtin(s) {
        return Ok(SchemeVal::Builtin(s.to_string()));
    }
    Err(SchemeErr::Undefined(s.to_string()))
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "+" | "-"
            | "*"
            | "/"
            | "mod"
            | "remainder"
            | "quotient"
            | "="
            | "<"
            | ">"
            | "<="
            | ">="
            | "equal?"
            | "eqv?"
            | "eq?"
            | "not"
            | "list"
            | "cons"
            | "car"
            | "cdr"
            | "append"
            | "reverse"
            | "length"
            | "list-ref"
            | "null?"
            | "pair?"
            | "string?"
            | "number?"
            | "boolean?"
            | "procedure?"
            | "string-append"
            | "string-length"
            | "substring"
            | "string-contains"
            | "string-index"
            | "string-upcase"
            | "string-downcase"
            | "number->string"
            | "string->number"
            | "abs"
            | "max"
            | "min"
            | "floor"
            | "ceiling"
            | "round"
            | "truncate"
            | "even?"
            | "odd?"
            | "zero?"
            | "positive?"
            | "negative?"
            | "map"
            | "filter"
            | "for-each"
            | "fold"
            | "fold-left"
            | "display"
            | "write"
            | "newline"
            | "error"
            | "assert"
            | "list?"
            | "rpc-send"
            | "msg-send"
            | "ok?"
            | "err?"
            | "ok-val"
            | "err-msg"
            | "use"
            | "include"
            | "cadr"
            | "caddr"
            | "cadddr"
    )
}

// ── Special forms ──────────────────────────────────────────────────────────

async fn eval_define(forms: Vec<SchemeExpr>, env: Env, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    if forms.len() < 3 {
        return Err(SchemeErr::Runtime(
            "define: expected name and value".to_string(),
        ));
    }
    match &forms[1] {
        SchemeExpr::Atom(name) => {
            let val = eval(forms[2].clone(), env.clone(), ctx).await?;
            env.define(name.clone(), val);
            Ok(SchemeVal::Nil)
        }
        SchemeExpr::List(params) => {
            if params.is_empty() {
                return Err(SchemeErr::Runtime(
                    "define: function form missing name".to_string(),
                ));
            }
            let name = atom_name(&params[0], "define")?;
            let param_names = params[1..]
                .iter()
                .map(|p| atom_name(p, "define"))
                .collect::<Result<Vec<_>, _>>()?;
            let (param_names, rest) = extract_rest_param(param_names);
            let body = forms[2..].to_vec();
            env.define(
                name,
                SchemeVal::Lambda {
                    params: param_names,
                    rest,
                    body,
                    env: env.clone(),
                },
            );
            Ok(SchemeVal::Nil)
        }
        _ => Err(SchemeErr::Runtime(
            "define: first arg must be a symbol or list".to_string(),
        )),
    }
}

fn eval_lambda(forms: Vec<SchemeExpr>, env: Env) -> Result<SchemeVal, SchemeErr> {
    if forms.len() < 3 {
        return Err(SchemeErr::Runtime(
            "lambda: expected parameter list and body".to_string(),
        ));
    }
    let param_names = match &forms[1] {
        SchemeExpr::List(ps) => ps
            .iter()
            .map(|p| atom_name(p, "lambda"))
            .collect::<Result<Vec<_>, _>>()?,
        SchemeExpr::Nil => vec![],
        SchemeExpr::Atom(s) if s == "()" || s == "nil" => vec![],
        _ => {
            return Err(SchemeErr::Runtime(
                "lambda: parameter list must be a list".to_string(),
            ))
        }
    };
    let (params, rest) = extract_rest_param(param_names);
    Ok(SchemeVal::Lambda {
        params,
        rest,
        body: forms[2..].to_vec(),
        env,
    })
}

async fn eval_let(
    forms: Vec<SchemeExpr>,
    env: Env,
    ctx: Ctx,
    star: bool,
) -> Result<SchemeVal, SchemeErr> {
    if forms.len() < 3 {
        return Err(SchemeErr::Runtime("let: expected bindings and body".to_string()));
    }
    let bindings = match &forms[1] {
        SchemeExpr::List(b) => b.clone(),
        _ => return Err(SchemeErr::Runtime("let: bindings must be a list".to_string())),
    };
    let new_env = if star {
        let e = Env::extend(&env);
        for binding in &bindings {
            let (name, val_expr) = let_binding(binding, "let*")?;
            let val = eval(val_expr, e.clone(), ctx.clone()).await?;
            e.define(name, val);
        }
        e
    } else {
        let e = Env::extend(&env);
        for binding in &bindings {
            let (name, val_expr) = let_binding(binding, "let")?;
            let val = eval(val_expr, env.clone(), ctx.clone()).await?;
            e.define(name, val);
        }
        e
    };
    eval_begin(&forms[2..], new_env, ctx).await
}

async fn eval_letrec(forms: Vec<SchemeExpr>, env: Env, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    if forms.len() < 3 {
        return Err(SchemeErr::Runtime(
            "letrec: expected bindings and body".to_string(),
        ));
    }
    let bindings = match &forms[1] {
        SchemeExpr::List(b) => b.clone(),
        _ => return Err(SchemeErr::Runtime("letrec: bindings must be a list".to_string())),
    };
    let new_env = Env::extend(&env);
    for binding in &bindings {
        let (name, _) = let_binding(binding, "letrec")?;
        new_env.define(name, SchemeVal::Nil);
    }
    for binding in &bindings {
        let (name, val_expr) = let_binding(binding, "letrec")?;
        let val = eval(val_expr, new_env.clone(), ctx.clone()).await?;
        new_env.define(name, val);
    }
    eval_begin(&forms[2..], new_env, ctx).await
}

async fn eval_if(forms: Vec<SchemeExpr>, env: Env, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    if forms.len() < 3 || forms.len() > 4 {
        return Err(SchemeErr::Runtime(
            "if: expected (if cond then) or (if cond then else)".to_string(),
        ));
    }
    let cond = eval(forms[1].clone(), env.clone(), ctx.clone()).await?;
    if cond.is_truthy() {
        eval(forms[2].clone(), env, ctx).await
    } else if forms.len() == 4 {
        eval(forms[3].clone(), env, ctx).await
    } else {
        Ok(SchemeVal::Nil)
    }
}

async fn eval_cond(forms: Vec<SchemeExpr>, env: Env, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    for clause in &forms[1..] {
        if let SchemeExpr::List(parts) = clause {
            if parts.is_empty() {
                continue;
            }
            if let SchemeExpr::Atom(kw) = &parts[0] {
                if kw == "else" {
                    return eval_begin(&parts[1..], env, ctx).await;
                }
            }
            let test = eval(parts[0].clone(), env.clone(), ctx.clone()).await?;
            if test.is_truthy() {
                return if parts.len() == 1 {
                    Ok(test)
                } else {
                    eval_begin(&parts[1..], env, ctx).await
                };
            }
        }
    }
    Ok(SchemeVal::Nil)
}

// ── Pipe threading ────────────────────────────────────────────────────────

fn eval_pipe(
    forms: Vec<SchemeExpr>,
    env: Env,
    ctx: Ctx,
) -> LocalBoxFuture<'static, Result<SchemeVal, SchemeErr>> {
    Box::pin(async move {
        let mut stages: Vec<Vec<SchemeExpr>> = vec![vec![]];
        for form in forms {
            if matches!(&form, SchemeExpr::Atom(s) if s == "|") {
                stages.push(vec![]);
            } else {
                stages.last_mut().unwrap().push(form);
            }
        }

        let first = stages.remove(0);
        let mut acc = if first.len() == 1 {
            eval(first[0].clone(), env.clone(), ctx.clone()).await?
        } else {
            let h = eval(first[0].clone(), env.clone(), ctx.clone()).await?;
            let mut a = Vec::with_capacity(first.len() - 1);
            for f in &first[1..] {
                a.push(eval(f.clone(), env.clone(), ctx.clone()).await?);
            }
            apply(h, a, ctx.clone()).await?
        };

        for stage in stages {
            if stage.is_empty() {
                continue;
            }
            let stage_env = Env::extend(&env);
            stage_env.define("_", acc.clone());

            let has_placeholder = stage
                .iter()
                .any(|f| matches!(f, SchemeExpr::Atom(s) if s == "_"));

            let f = eval(stage[0].clone(), stage_env.clone(), ctx.clone()).await?;

            let mut args = if has_placeholder {
                Vec::with_capacity(stage.len() - 1)
            } else {
                vec![acc]
            };
            for form in &stage[1..] {
                args.push(eval(form.clone(), stage_env.clone(), ctx.clone()).await?);
            }

            acc = apply(f, args, ctx.clone()).await?;
        }

        Ok(acc)
    })
}

fn eval_begin(
    forms: &[SchemeExpr],
    env: Env,
    ctx: Ctx,
) -> LocalBoxFuture<'static, Result<SchemeVal, SchemeErr>> {
    let forms = forms.to_owned();
    Box::pin(async move {
        if forms.is_empty() {
            return Ok(SchemeVal::Nil);
        }
        let last = forms.len() - 1;
        for form in &forms[..last] {
            eval(form.clone(), env.clone(), ctx.clone()).await?;
        }
        eval(forms[last].clone(), env, ctx).await
    })
}

async fn eval_and(forms: &[SchemeExpr], env: Env, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    let mut last = SchemeVal::Bool(true);
    for form in forms {
        let v = eval(form.clone(), env.clone(), ctx.clone()).await?;
        if !v.is_truthy() {
            return Ok(SchemeVal::Bool(false));
        }
        last = v;
    }
    Ok(last)
}

async fn eval_or(forms: &[SchemeExpr], env: Env, ctx: Ctx) -> Result<SchemeVal, SchemeErr> {
    for form in forms {
        let v = eval(form.clone(), env.clone(), ctx.clone()).await?;
        if v.is_truthy() {
            return Ok(v);
        }
    }
    Ok(SchemeVal::Bool(false))
}

// ── Application ────────────────────────────────────────────────────────────

fn apply(
    func: SchemeVal,
    args: Vec<SchemeVal>,
    ctx: Ctx,
) -> LocalBoxFuture<'static, Result<SchemeVal, SchemeErr>> {
    Box::pin(async move {
        match func {
            SchemeVal::Lambda {
                params,
                rest,
                body,
                env,
            } => {
                let new_env = Env::extend(&env);
                let min = params.len();
                if rest.is_none() && args.len() != min {
                    return Err(SchemeErr::Arity {
                        name: "#<lambda>".to_string(),
                        expected: min,
                        got: args.len(),
                    });
                }
                if args.len() < min {
                    return Err(SchemeErr::Arity {
                        name: "#<lambda>".to_string(),
                        expected: min,
                        got: args.len(),
                    });
                }
                for (p, a) in params.iter().zip(args.iter()) {
                    new_env.define(p.clone(), a.clone());
                }
                if let Some(rest_name) = rest {
                    new_env.define(rest_name, SchemeVal::List(args[min..].to_vec()));
                }
                eval_begin(&body, new_env, ctx).await
            }

            SchemeVal::Builtin(name) => apply_builtin(name, args, ctx).await,

            SchemeVal::MaPath(path) => {
                let command = if args.len() == 1 && args[0].to_splice_lossy().is_empty() {
                    let clean = path.trim_end_matches(':');
                    format!("{clean}:")
                } else if args.is_empty() {
                    path.clone()
                } else {
                    let args_str = args
                        .iter()
                        .map(|v| v.to_splice_lossy())
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("{path} {args_str}")
                };
                ctx.eval_dot(&command)
            }

            SchemeVal::MaActor(actor) => {
                let cmd = reconstruct_actor_command(&actor, &args);
                ctx.eval_actor(&cmd).await
            }

            // DID string in function position.
            SchemeVal::Str(ref s) if s.starts_with("did:") => {
                let cmd = reconstruct_actor_command(s, &args);
                ctx.eval_actor(&cmd).await
            }

            other => Err(SchemeErr::Runtime(format!(
                "not a procedure: {}",
                other.display()
            ))),
        }
    })
}

fn reconstruct_actor_command(target: &str, args: &[SchemeVal]) -> String {
    let mut result = target.to_string();
    let mut iter = args.iter();

    if let Some(first) = iter.next() {
        let s = first.to_splice_lossy();
        if !target.contains('#') && s.starts_with('#') {
            result.push_str(&s);
        } else {
            result.push(' ');
            result.push_str(&s);
        }
    }
    for arg in iter {
        result.push(' ');
        result.push_str(&arg.to_splice_lossy());
    }
    result
}

// ── Builtins ───────────────────────────────────────────────────────────────

fn apply_builtin(
    name: String,
    args: Vec<SchemeVal>,
    ctx: Ctx,
) -> LocalBoxFuture<'static, Result<SchemeVal, SchemeErr>> {
    Box::pin(async move {
        match name.as_str() {
            "+" => {
                let mut sum_i: i64 = 0;
                let mut sum_f: f64 = 0.0;
                let mut is_float = false;
                for a in &args {
                    match a {
                        SchemeVal::Int(n) => sum_i += n,
                        SchemeVal::Float(f) => { is_float = true; sum_f += f; }
                        _ => return Err(SchemeErr::Runtime(format!("+: not a number: {}", a.display()))),
                    }
                }
                if is_float { Ok(SchemeVal::Float(sum_f + sum_i as f64)) } else { Ok(SchemeVal::Int(sum_i)) }
            }
            "-" => {
                arity_min("-", &args, 1)?;
                match &args[0] {
                    SchemeVal::Int(n) if args.len() == 1 => Ok(SchemeVal::Int(-n)),
                    SchemeVal::Float(f) if args.len() == 1 => Ok(SchemeVal::Float(-f)),
                    SchemeVal::Int(first) => {
                        let mut r = *first;
                        for a in &args[1..] {
                            match a {
                                SchemeVal::Int(n) => r -= n,
                                _ => return Err(SchemeErr::Runtime("-: not an integer".into())),
                            }
                        }
                        Ok(SchemeVal::Int(r))
                    }
                    SchemeVal::Float(first) => {
                        let mut r = *first;
                        for a in &args[1..] {
                            match a {
                                SchemeVal::Float(f) => r -= f,
                                SchemeVal::Int(n) => r -= *n as f64,
                                _ => return Err(SchemeErr::Runtime("-: not a number".into())),
                            }
                        }
                        Ok(SchemeVal::Float(r))
                    }
                    _ => Err(SchemeErr::Runtime("-: not a number".into())),
                }
            }
            "*" => {
                let mut prod_i: i64 = 1;
                let mut prod_f: f64 = 1.0;
                let mut is_float = false;
                for a in &args {
                    match a {
                        SchemeVal::Int(n) => prod_i *= n,
                        SchemeVal::Float(f) => { is_float = true; prod_f *= f; }
                        _ => return Err(SchemeErr::Runtime(format!("*: not a number: {}", a.display()))),
                    }
                }
                if is_float { Ok(SchemeVal::Float(prod_f * prod_i as f64)) } else { Ok(SchemeVal::Int(prod_i)) }
            }
            "/" => {
                arity("/", &args, 2)?;
                match (&args[0], &args[1]) {
                    (SchemeVal::Int(a), SchemeVal::Int(b)) => {
                        if *b == 0 { return Err(SchemeErr::Runtime("division by zero".into())); }
                        Ok(SchemeVal::Int(a / b))
                    }
                    (SchemeVal::Float(a), SchemeVal::Float(b)) => Ok(SchemeVal::Float(a / b)),
                    (SchemeVal::Int(a), SchemeVal::Float(b)) => Ok(SchemeVal::Float(*a as f64 / b)),
                    (SchemeVal::Float(a), SchemeVal::Int(b)) => Ok(SchemeVal::Float(a / *b as f64)),
                    _ => Err(SchemeErr::Runtime("/: not numbers".into())),
                }
            }
            "mod" | "remainder" => {
                arity("mod", &args, 2)?;
                match (&args[0], &args[1]) {
                    (SchemeVal::Int(a), SchemeVal::Int(b)) => {
                        if *b == 0 { return Err(SchemeErr::Runtime("modulo by zero".into())); }
                        Ok(SchemeVal::Int(a % b))
                    }
                    _ => Err(SchemeErr::Runtime("mod: not integers".into())),
                }
            }
            "quotient" => {
                arity("quotient", &args, 2)?;
                match (&args[0], &args[1]) {
                    (SchemeVal::Int(a), SchemeVal::Int(b)) => {
                        if *b == 0 { return Err(SchemeErr::Runtime("quotient: division by zero".into())); }
                        Ok(SchemeVal::Int(a / b))
                    }
                    _ => Err(SchemeErr::Runtime("quotient: not integers".into())),
                }
            }
            "=" | "equal?" | "eqv?" | "eq?" => {
                arity_min("=", &args, 2)?;
                let first = &args[0];
                for a in &args[1..] {
                    if !scheme_equal(first, a) { return Ok(SchemeVal::Bool(false)); }
                }
                Ok(SchemeVal::Bool(true))
            }
            "<" => compare_chain(&args, num_lt),
            ">" => compare_chain(&args, |a, b| num_lt(b, a)),
            "<=" => compare_chain(&args, |a, b| num_lt(b, a).map(|r| !r)),
            ">=" => compare_chain(&args, |a, b| num_lt(a, b).map(|r| !r)),
            "not" => {
                arity("not", &args, 1)?;
                Ok(SchemeVal::Bool(!args[0].is_truthy()))
            }
            "list" => Ok(SchemeVal::List(args)),
            "cons" => {
                arity("cons", &args, 2)?;
                match args[1].clone() {
                    SchemeVal::List(mut v) => { v.insert(0, args[0].clone()); Ok(SchemeVal::List(v)) }
                    SchemeVal::Nil => Ok(SchemeVal::List(vec![args[0].clone()])),
                    b => Ok(SchemeVal::List(vec![args[0].clone(), b])),
                }
            }
            "car" => {
                arity("car", &args, 1)?;
                match &args[0] {
                    SchemeVal::List(v) if !v.is_empty() => Ok(v[0].clone()),
                    _ => Err(SchemeErr::Runtime("car: not a pair".into())),
                }
            }
            "cdr" => {
                arity("cdr", &args, 1)?;
                match &args[0] {
                    SchemeVal::List(v) if v.len() > 1 => Ok(SchemeVal::List(v[1..].to_vec())),
                    SchemeVal::List(_) => Ok(SchemeVal::Nil),
                    _ => Err(SchemeErr::Runtime("cdr: not a pair".into())),
                }
            }
            "append" => {
                let mut result = Vec::new();
                for a in args {
                    match a {
                        SchemeVal::List(v) => result.extend(v),
                        SchemeVal::Nil => {}
                        _ => return Err(SchemeErr::Runtime("append: not a list".into())),
                    }
                }
                Ok(SchemeVal::List(result))
            }
            "reverse" => {
                arity("reverse", &args, 1)?;
                match &args[0] {
                    SchemeVal::List(v) => Ok(SchemeVal::List(v.iter().rev().cloned().collect())),
                    SchemeVal::Nil => Ok(SchemeVal::Nil),
                    _ => Err(SchemeErr::Runtime("reverse: not a list".into())),
                }
            }
            "length" => {
                arity("length", &args, 1)?;
                match &args[0] {
                    SchemeVal::List(v) => Ok(SchemeVal::Int(v.len() as i64)),
                    SchemeVal::Nil => Ok(SchemeVal::Int(0)),
                    _ => Err(SchemeErr::Runtime("length: not a list".into())),
                }
            }
            "list-ref" => {
                arity("list-ref", &args, 2)?;
                let lst = match &args[0] {
                    SchemeVal::List(v) => v,
                    _ => return Err(SchemeErr::Runtime("list-ref: not a list".into())),
                };
                let idx = match &args[1] {
                    SchemeVal::Int(n) => *n as usize,
                    _ => return Err(SchemeErr::Runtime("list-ref: index not an integer".into())),
                };
                lst.get(idx)
                    .cloned()
                    .ok_or_else(|| SchemeErr::Runtime(format!("list-ref: index {idx} out of range")))
            }
            "null?" => {
                arity("null?", &args, 1)?;
                let is_null = match &args[0] {
                    SchemeVal::Nil => true,
                    SchemeVal::List(v) if v.is_empty() => true,
                    _ => false,
                };
                Ok(SchemeVal::Bool(is_null))
            }
            "pair?" => {
                arity("pair?", &args, 1)?;
                Ok(SchemeVal::Bool(matches!(&args[0], SchemeVal::List(v) if !v.is_empty())))
            }
            "string?" => { arity("string?", &args, 1)?; Ok(SchemeVal::Bool(matches!(&args[0], SchemeVal::Str(_)))) }
            "number?" => { arity("number?", &args, 1)?; Ok(SchemeVal::Bool(matches!(&args[0], SchemeVal::Int(_) | SchemeVal::Float(_)))) }
            "boolean?" => { arity("boolean?", &args, 1)?; Ok(SchemeVal::Bool(matches!(&args[0], SchemeVal::Bool(_)))) }
            "procedure?" => {
                arity("procedure?", &args, 1)?;
                Ok(SchemeVal::Bool(matches!(&args[0], SchemeVal::Lambda { .. } | SchemeVal::Builtin(_))))
            }
            "string-append" => {
                let mut s = String::new();
                for a in &args {
                    match a {
                        SchemeVal::Str(st) => s.push_str(st),
                        _ => return Err(SchemeErr::Runtime("string-append: not a string".into())),
                    }
                }
                Ok(SchemeVal::Str(s))
            }
            "string-length" => {
                arity("string-length", &args, 1)?;
                match &args[0] {
                    SchemeVal::Str(s) => Ok(SchemeVal::Int(s.len() as i64)),
                    _ => Err(SchemeErr::Runtime("string-length: not a string".into())),
                }
            }
            "substring" => {
                arity("substring", &args, 3)?;
                let s = match &args[0] {
                    SchemeVal::Str(s) => s.clone(),
                    _ => return Err(SchemeErr::Runtime("substring: not a string".into())),
                };
                let start = int_arg(&args[1], "substring")?;
                let end = int_arg(&args[2], "substring")?;
                Ok(SchemeVal::Str(s.get(start..end).unwrap_or("").to_string()))
            }
            "string-contains" => {
                arity("string-contains", &args, 2)?;
                match (&args[0], &args[1]) {
                    (SchemeVal::Str(hay), SchemeVal::Str(needle)) => Ok(SchemeVal::Bool(hay.contains(needle.as_str()))),
                    _ => Err(SchemeErr::Runtime("string-contains: not strings".into())),
                }
            }
            "string-index" => {
                arity("string-index", &args, 2)?;
                match (&args[0], &args[1]) {
                    (SchemeVal::Str(hay), SchemeVal::Str(needle)) => Ok(match hay.find(needle.as_str()) {
                        Some(i) => SchemeVal::Int(i as i64),
                        None => SchemeVal::Bool(false),
                    }),
                    _ => Err(SchemeErr::Runtime("string-index: not strings".into())),
                }
            }
            "cadr" => {
                arity("cadr", &args, 1)?;
                match &args[0] {
                    SchemeVal::List(v) if v.len() >= 2 => Ok(v[1].clone()),
                    _ => Err(SchemeErr::Runtime("cadr: list too short".into())),
                }
            }
            "caddr" => {
                arity("caddr", &args, 1)?;
                match &args[0] {
                    SchemeVal::List(v) if v.len() >= 3 => Ok(v[2].clone()),
                    _ => Err(SchemeErr::Runtime("caddr: list too short".into())),
                }
            }
            "cadddr" => {
                arity("cadddr", &args, 1)?;
                match &args[0] {
                    SchemeVal::List(v) if v.len() >= 4 => Ok(v[3].clone()),
                    _ => Err(SchemeErr::Runtime("cadddr: list too short".into())),
                }
            }
            "string-upcase" => {
                arity("string-upcase", &args, 1)?;
                match &args[0] {
                    SchemeVal::Str(s) => Ok(SchemeVal::Str(s.to_uppercase())),
                    _ => Err(SchemeErr::Runtime("string-upcase: not a string".into())),
                }
            }
            "string-downcase" => {
                arity("string-downcase", &args, 1)?;
                match &args[0] {
                    SchemeVal::Str(s) => Ok(SchemeVal::Str(s.to_lowercase())),
                    _ => Err(SchemeErr::Runtime("string-downcase: not a string".into())),
                }
            }
            "number->string" => {
                arity("number->string", &args, 1)?;
                match &args[0] {
                    SchemeVal::Int(n) => Ok(SchemeVal::Str(n.to_string())),
                    SchemeVal::Float(f) => Ok(SchemeVal::Str(f.to_string())),
                    _ => Err(SchemeErr::Runtime("number->string: not a number".into())),
                }
            }
            "string->number" => {
                arity("string->number", &args, 1)?;
                match &args[0] {
                    SchemeVal::Str(s) => Ok(if let Ok(n) = s.parse::<i64>() {
                        SchemeVal::Int(n)
                    } else if let Ok(f) = s.parse::<f64>() {
                        SchemeVal::Float(f)
                    } else {
                        SchemeVal::Bool(false)
                    }),
                    _ => Err(SchemeErr::Runtime("string->number: not a string".into())),
                }
            }
            "abs" => {
                arity("abs", &args, 1)?;
                match &args[0] {
                    SchemeVal::Int(n) => Ok(SchemeVal::Int(n.abs())),
                    SchemeVal::Float(f) => Ok(SchemeVal::Float(f.abs())),
                    _ => Err(SchemeErr::Runtime("abs: not a number".into())),
                }
            }
            "max" => {
                arity_min("max", &args, 1)?;
                let mut m = args[0].clone();
                for a in &args[1..] {
                    if num_lt(&m, a)? { m = a.clone(); }
                }
                Ok(m)
            }
            "min" => {
                arity_min("min", &args, 1)?;
                let mut m = args[0].clone();
                for a in &args[1..] {
                    if num_lt(a, &m)? { m = a.clone(); }
                }
                Ok(m)
            }
            "floor" => one_float("floor", &args, f64::floor),
            "ceiling" => one_float("ceiling", &args, f64::ceil),
            "round" => one_float("round", &args, f64::round),
            "truncate" => one_float("truncate", &args, f64::trunc),
            "even?" => {
                arity("even?", &args, 1)?;
                match &args[0] {
                    SchemeVal::Int(n) => Ok(SchemeVal::Bool(n % 2 == 0)),
                    _ => Err(SchemeErr::Runtime("even?: not an integer".into())),
                }
            }
            "odd?" => {
                arity("odd?", &args, 1)?;
                match &args[0] {
                    SchemeVal::Int(n) => Ok(SchemeVal::Bool(n % 2 != 0)),
                    _ => Err(SchemeErr::Runtime("odd?: not an integer".into())),
                }
            }
            "zero?" => num_pred("zero?", &args, |i| i == 0, |f| f == 0.0),
            "positive?" => num_pred("positive?", &args, |i| i > 0, |f| f > 0.0),
            "negative?" => num_pred("negative?", &args, |i| i < 0, |f| f < 0.0),
            "map" => {
                arity_min("map", &args, 2)?;
                let f = args[0].clone();
                let lst = list_arg(&args[1], "map")?;
                let mut result = Vec::with_capacity(lst.len());
                for item in lst {
                    result.push(apply(f.clone(), vec![item], ctx.clone()).await?);
                }
                Ok(SchemeVal::List(result))
            }
            "filter" => {
                arity("filter", &args, 2)?;
                let f = args[0].clone();
                let lst = list_arg(&args[1], "filter")?;
                let mut result = Vec::new();
                for item in lst {
                    let keep = apply(f.clone(), vec![item.clone()], ctx.clone()).await?;
                    if keep.is_truthy() { result.push(item); }
                }
                Ok(SchemeVal::List(result))
            }
            "for-each" => {
                arity_min("for-each", &args, 2)?;
                let f = args[0].clone();
                let lst = list_arg(&args[1], "for-each")?;
                for item in lst {
                    apply(f.clone(), vec![item], ctx.clone()).await?;
                }
                Ok(SchemeVal::Nil)
            }
            "fold" | "fold-left" => {
                arity("fold", &args, 3)?;
                let f = args[0].clone();
                let mut acc = args[1].clone();
                let lst = list_arg(&args[2], "fold")?;
                for item in lst {
                    acc = apply(f.clone(), vec![item, acc], ctx.clone()).await?;
                }
                Ok(acc)
            }
            // I/O — write to stdout
            "display" | "write" => {
                let text = if name == "write" {
                    args.iter().map(|v| v.repr()).collect::<Vec<_>>().join(" ")
                } else {
                    args.iter().map(|v| v.display()).collect::<Vec<_>>().join(" ")
                };
                ctx.display_output(&text);
                Ok(SchemeVal::Nil)
            }
            "newline" => {
                ctx.display_output("\n");
                Ok(SchemeVal::Nil)
            }
            "error" => {
                let msg = args.iter().map(|v| v.display()).collect::<Vec<_>>().join(" ");
                Err(SchemeErr::Runtime(msg))
            }
            "assert" => {
                arity("assert", &args, 1)?;
                if args[0].is_truthy() {
                    Ok(args[0].clone())
                } else {
                    Err(SchemeErr::Runtime("assertion failed".into()))
                }
            }
            "list?" => {
                arity("list?", &args, 1)?;
                Ok(SchemeVal::Bool(matches!(&args[0], SchemeVal::List(_) | SchemeVal::Nil)))
            }
            // ── ma send primitives ────────────────────────────────────────
            "rpc-send" => {
                arity_min("rpc-send", &args, 2)?;
                let raw = str_arg(&args[0], "rpc-send")?;
                let verb = str_arg(&args[1], "rpc-send")?;
                let target = ctx.resolve_target(&raw).map_err(SchemeErr::MaError)?;
                let extra: Vec<String> = args[2..].iter().map(|v| v.to_splice_lossy()).collect();

                let send_result = ctx.send_rpc(&target, &verb, &extra).await;
                match send_result {
                    Err(e) => Ok(err_tuple(e)),
                    Ok(msg_id) => {
                        let (sender, receiver) =
                            futures::channel::oneshot::channel::<Result<String, String>>();
                        ctx.register_reply_sender(msg_id, sender);
                        match receiver.await {
                            Ok(Ok(content)) => Ok(ok_tuple(content)),
                            Ok(Err(e)) => Ok(err_tuple(e)),
                            Err(_) => Ok(timeout_tuple()),
                        }
                    }
                }
            }
            "msg-send" => {
                arity("msg-send", &args, 2)?;
                let raw = str_arg(&args[0], "msg-send")?;
                let body = str_arg(&args[1], "msg-send")?;
                let target = ctx.resolve_target(&raw).map_err(SchemeErr::MaError)?;
                match ctx.send_text(&target, &body).await {
                    Ok(msg_id) => Ok(ok_tuple(msg_id)),
                    Err(e) => Ok(err_tuple(e)),
                }
            }
            // ── Reply tuple helpers ────────────────────────────────────────
            "ok?" => {
                arity("ok?", &args, 1)?;
                Ok(SchemeVal::Bool(is_ok_tuple(&args[0])))
            }
            "err?" => {
                arity("err?", &args, 1)?;
                Ok(SchemeVal::Bool(is_err_tuple(&args[0])))
            }
            "ok-val" => {
                arity("ok-val", &args, 1)?;
                match &args[0] {
                    SchemeVal::List(v)
                        if v.len() >= 2 && matches!(&v[0], SchemeVal::Str(s) if s == ":ok") =>
                    {
                        Ok(v[1].clone())
                    }
                    _ => Err(SchemeErr::Runtime("ok-val: not an (:ok value) tuple".into())),
                }
            }
            "err-msg" => {
                arity("err-msg", &args, 1)?;
                match &args[0] {
                    SchemeVal::List(v)
                        if v.len() >= 2 && matches!(&v[0], SchemeVal::Str(s) if s == ":error") =>
                    {
                        Ok(v[1].clone())
                    }
                    _ => Err(SchemeErr::Runtime("err-msg: not an (:error reason) tuple".into())),
                }
            }
            "use" => {
                // Focus mode not applicable in CLI scripts — no-op.
                tracing::debug!("use: focus mode is a no-op in CLI context");
                Ok(SchemeVal::Nil)
            }
            "include" => {
                arity("include", &args, 1)?;
                let path = match &args[0] {
                    SchemeVal::Str(s) => s.clone(),
                    other => {
                        return Err(SchemeErr::Runtime(format!(
                            "include: expected a path string or CID, got {}",
                            other.display()
                        )))
                    }
                };
                let content = if path.starts_with('.') {
                    ctx.config_get(&path).ok_or_else(|| {
                        SchemeErr::MaError(format!("include: no value at {path}"))
                    })?
                } else if is_link_value(&path) {
                    ctx.fetch_cid(&path).await.map_err(SchemeErr::MaError)?
                } else {
                    path.clone()
                };
                ctx.eval_include(&content).await
            }
            other => Err(SchemeErr::Undefined(other.to_string())),
        }
    })
}

// ── Helper utilities ───────────────────────────────────────────────────────

fn arity(name: &str, args: &[SchemeVal], n: usize) -> Result<(), SchemeErr> {
    if args.len() != n {
        Err(SchemeErr::Arity { name: name.to_string(), expected: n, got: args.len() })
    } else {
        Ok(())
    }
}

fn arity_min(name: &str, args: &[SchemeVal], min: usize) -> Result<(), SchemeErr> {
    if args.len() < min {
        Err(SchemeErr::Arity { name: name.to_string(), expected: min, got: args.len() })
    } else {
        Ok(())
    }
}

fn scheme_equal(a: &SchemeVal, b: &SchemeVal) -> bool {
    match (a, b) {
        (SchemeVal::Int(x), SchemeVal::Int(y)) => x == y,
        (SchemeVal::Float(x), SchemeVal::Float(y)) => x == y,
        (SchemeVal::Int(x), SchemeVal::Float(y)) => (*x as f64) == *y,
        (SchemeVal::Float(x), SchemeVal::Int(y)) => *x == (*y as f64),
        (SchemeVal::Str(x), SchemeVal::Str(y)) => x == y,
        (SchemeVal::Bool(x), SchemeVal::Bool(y)) => x == y,
        (SchemeVal::Nil, SchemeVal::Nil) => true,
        (SchemeVal::List(x), SchemeVal::List(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| scheme_equal(a, b))
        }
        _ => false,
    }
}

fn num_lt(a: &SchemeVal, b: &SchemeVal) -> Result<bool, SchemeErr> {
    match (a, b) {
        (SchemeVal::Int(x), SchemeVal::Int(y)) => Ok(x < y),
        (SchemeVal::Float(x), SchemeVal::Float(y)) => Ok(x < y),
        (SchemeVal::Int(x), SchemeVal::Float(y)) => Ok((*x as f64) < *y),
        (SchemeVal::Float(x), SchemeVal::Int(y)) => Ok(*x < (*y as f64)),
        _ => Err(SchemeErr::Runtime(format!(
            "comparison: not numbers: {} and {}",
            a.display(),
            b.display()
        ))),
    }
}

fn compare_chain(
    args: &[SchemeVal],
    cmp: impl Fn(&SchemeVal, &SchemeVal) -> Result<bool, SchemeErr>,
) -> Result<SchemeVal, SchemeErr> {
    if args.len() < 2 {
        return Err(SchemeErr::Arity { name: "comparison".to_string(), expected: 2, got: args.len() });
    }
    for pair in args.windows(2) {
        if !cmp(&pair[0], &pair[1])? { return Ok(SchemeVal::Bool(false)); }
    }
    Ok(SchemeVal::Bool(true))
}

fn one_float(name: &str, args: &[SchemeVal], f: fn(f64) -> f64) -> Result<SchemeVal, SchemeErr> {
    arity(name, args, 1)?;
    match &args[0] {
        SchemeVal::Int(n) => Ok(SchemeVal::Int(*n)),
        SchemeVal::Float(v) => Ok(SchemeVal::Float(f(*v))),
        _ => Err(SchemeErr::Runtime(format!("{name}: not a number"))),
    }
}

fn num_pred(
    name: &str,
    args: &[SchemeVal],
    int_pred: fn(i64) -> bool,
    float_pred: fn(f64) -> bool,
) -> Result<SchemeVal, SchemeErr> {
    arity(name, args, 1)?;
    match &args[0] {
        SchemeVal::Int(n) => Ok(SchemeVal::Bool(int_pred(*n))),
        SchemeVal::Float(f) => Ok(SchemeVal::Bool(float_pred(*f))),
        _ => Err(SchemeErr::Runtime(format!("{name}: not a number"))),
    }
}

fn list_arg(v: &SchemeVal, name: &str) -> Result<Vec<SchemeVal>, SchemeErr> {
    match v {
        SchemeVal::List(items) => Ok(items.clone()),
        SchemeVal::Nil => Ok(vec![]),
        _ => Err(SchemeErr::Runtime(format!("{name}: not a list"))),
    }
}

fn int_arg(v: &SchemeVal, name: &str) -> Result<usize, SchemeErr> {
    match v {
        SchemeVal::Int(n) => Ok(*n as usize),
        _ => Err(SchemeErr::Runtime(format!("{name}: index not an integer"))),
    }
}

fn atom_name(expr: &SchemeExpr, ctx: &str) -> Result<String, SchemeErr> {
    match expr {
        SchemeExpr::Atom(n) => Ok(n.clone()),
        _ => Err(SchemeErr::Runtime(format!("{ctx}: expected a symbol, got {expr:?}"))),
    }
}

fn let_binding(binding: &SchemeExpr, ctx: &str) -> Result<(String, SchemeExpr), SchemeErr> {
    match binding {
        SchemeExpr::List(pair) if pair.len() == 2 => {
            let name = atom_name(&pair[0], ctx)?;
            Ok((name, pair[1].clone()))
        }
        _ => Err(SchemeErr::Runtime(format!("{ctx}: each binding must be (name value)"))),
    }
}

fn extract_rest_param(mut params: Vec<String>) -> (Vec<String>, Option<String>) {
    if let Some(dot_pos) = params.iter().position(|p| p == ".") {
        if dot_pos + 1 < params.len() {
            let rest = params.remove(dot_pos + 1);
            params.remove(dot_pos);
            return (params, Some(rest));
        }
    }
    (params, None)
}

fn expr_to_val(expr: &SchemeExpr) -> SchemeVal {
    match expr {
        SchemeExpr::Nil => SchemeVal::Nil,
        SchemeExpr::Str(s) => SchemeVal::Str(s.clone()),
        SchemeExpr::Atom(s) => SchemeVal::Str(s.clone()),
        SchemeExpr::List(forms) => SchemeVal::List(forms.iter().map(expr_to_val).collect()),
    }
}

// ── Reply tuple constructors ───────────────────────────────────────────────

fn ok_tuple(value: impl Into<String>) -> SchemeVal {
    SchemeVal::List(vec![
        SchemeVal::Str(":ok".to_string()),
        SchemeVal::Str(value.into()),
    ])
}

fn err_tuple(reason: impl Into<String>) -> SchemeVal {
    SchemeVal::List(vec![
        SchemeVal::Str(":error".to_string()),
        SchemeVal::Str(reason.into()),
    ])
}

fn timeout_tuple() -> SchemeVal {
    SchemeVal::List(vec![SchemeVal::Str(":timeout".to_string())])
}

fn is_ok_tuple(v: &SchemeVal) -> bool {
    matches!(v, SchemeVal::List(items)
        if matches!(items.first(), Some(SchemeVal::Str(s)) if s == ":ok"))
}

fn is_err_tuple(v: &SchemeVal) -> bool {
    matches!(v, SchemeVal::List(items)
        if matches!(items.first(), Some(SchemeVal::Str(s)) if s == ":error"))
}

fn str_arg(v: &SchemeVal, name: &str) -> Result<String, SchemeErr> {
    match v {
        SchemeVal::Str(s) => Ok(s.clone()),
        other => Err(SchemeErr::Runtime(format!(
            "{name}: expected a string, got {}",
            other.display()
        ))),
    }
}
