# zscheme Reference

**Version:** 0.1.0  
**Status:** Draft

Complete reference for the zscheme language as implemented in `zion`.

---

## Table of contents

1. [Syntax](#1-syntax)
2. [Types](#2-types)
3. [Special forms](#3-special-forms)
4. [ma primitives](#4-ma-primitives)
5. [Core builtins](#5-core-builtins)
6. [Send primitives](#6-send-primitives)
7. [Reply tuple helpers](#7-reply-tuple-helpers)
8. [Session environment](#8-session-environment)
9. [Limitations](#9-limitations)

---

## 1. Syntax

Any command line in `zion` containing `(` is pre-processed by the evaluator.
Each `(…)` span is evaluated as a Scheme expression; the result is substituted
as a string at that position. The remaining text is dispatched normally.

```
.path            → dot-command (config get/set/delete)
.path:func args  → Scheme function call (dispatches to .content)
.path!verb args  → system/side-effect operation (edit, eval, publish, save, …)
@                → actor message (RPC)
(                → Scheme expression
val | (f arg)    → pipe / threading (inside expressions)
```

All three dot/actor/scheme forms may appear in a single line:

```
(.my.aliases.sky)#room:enter ((.my.aliases.ms)#house:enter #room)
```

The `'` quote shorthand is supported: `'(a b c)` ≡ `(quote (a b c))`.

---

## 2. Types

| Type | Literal examples | Notes |
|---|---|---|
| Integer | `42`, `-7` | 64-bit signed |
| Float | `3.14`, `-0.5` | 64-bit IEEE 754 |
| String | `"hello"`, `"did:ma:…"` | UTF-8 |
| Boolean | `#t`, `#f` | |
| Nil | `()`, `nil` | Empty list / null |
| List | `(1 2 3)` | Proper list |
| Lambda | `(lambda (x) x)` | Closure |
| MaPath | `.my.aliases.sky` | Dot-path reference |
| MaActor | `@sky#house` | Actor target |

Fragment atoms such as `#room` and `#house:enter` are treated as strings.

---

## 3. Special forms

### `define`

```scheme
(define name value)
(define (name param…) body…)          ; shorthand for lambda
(define (name param… . rest) body…)  ; variadic
```

### `lambda` / `ʎ`

```scheme
(lambda (param…) body…)
(ʎ (param…) body…)                   ; Unicode alias
(lambda (a b . rest) body…)          ; variadic rest parameter
```

### `let` / `let*` / `letrec`

```scheme
(let    ((x 1) (y 2)) (+ x y))       ; parallel bindings
(let*   ((x 1) (y x)) (+ x y))       ; sequential bindings
(letrec ((f (lambda (n) …))) (f 10)) ; mutually recursive
```

### `if` / `cond` / `when` / `unless`

```scheme
(if cond then)
(if cond then else)
(cond (test expr…) … (else expr…))
(when  cond body…)
(unless cond body…)
```

### `begin`

```scheme
(begin expr…)                         ; sequence; returns last value
```

### `and` / `or`

```scheme
(and expr…)    ; short-circuits on #f; returns last truthy value
(or  expr…)    ; short-circuits on first truthy value
```

### `set!`

```scheme
(set! name value)                     ; mutate existing binding
```

### `quote`

```scheme
(quote expr)
'expr                                 ; shorthand
```

---

## 4. ma primitives

The evaluator recognises two dispatch classes based on the head of a list
form. These use the **existing ma grammar** — no new function names.

### Dot-path commands — head starts with `.`

```scheme
(.my.aliases.sky)           ; get leaf value → String
(.my.doc.notes.content)     ; get leaf value
(.my.config.k: "v")         ; set leaf       → Nil
(.my.aliases.old:)          ; delete subtree → Nil
```

If the path names a subtree rather than a leaf, a List of child path strings
is returned.

Dot-path verbs (`.path:verb`) are **not** supported inside Scheme expressions.

### Actor messages — head starts with `@` or evaluates to `did:…`

```scheme
(@sky#house:enter #room)              ; atom target, auto-unwraps reply
(did:ma:abc#room:enter ticket-xyz)    ; DID string in function position
```

When the head evaluates to a `did:…` string and the first argument starts
with `#`, the argument is appended without a space to form the fragment address:

```scheme
(define sky (.my.aliases.sky))        ; → "did:ma:abc"
(sky "#room:enter" ticket)            ; → sends to did:ma:abc#room:enter
```

The `@` actor syntax auto-unwraps replies: success returns `String`,
failure raises `SchemeErr`. Use `rpc-send` for explicit tuple handling.

---

## 5. Core builtins

### Arithmetic

| Function | Description |
|---|---|
| `(+ n…)` | Sum |
| `(- n…)` | Difference; unary negation |
| `(* n…)` | Product |
| `(/ a b)` | Division (integer or float) |
| `(mod a b)` | Modulo (integers) |
| `(floor n)` | Floor (returns Int for Int input) |
| `(ceiling n)` | Ceiling |
| `(round n)` | Round to nearest |
| `(truncate n)` | Truncate toward zero |

### Comparison

| Function | Description |
|---|---|
| `(= a b…)` | Numeric or structural equality |
| `(< a b…)` | Less-than chain |
| `(> a b…)` | Greater-than chain |
| `(<= a b…)` | Less-than-or-equal chain |
| `(>= a b…)` | Greater-than-or-equal chain |
| `(equal? a b)` | Deep equality |

### Boolean

| Function | Description |
|---|---|
| `(not v)` | Logical negation |

### Lists

| Function | Description |
|---|---|
| `(list v…)` | Construct list |
| `(cons a b)` | Prepend element |
| `(car lst)` | First element |
| `(cdr lst)` | Rest (tail) |
| `(null? v)` | True for `()` and nil |
| `(pair? v)` | True for non-empty list |

### Type predicates

| Function | Description |
|---|---|
| `(string? v)` | True for strings |
| `(number? v)` | True for integers and floats |
| `(boolean? v)` | True for `#t` / `#f` |
| `(procedure? v)` | True for lambdas and builtins |

### Strings (core primitives)

| Function | Description |
|---|---|
| `(string-append s…)` | Concatenate strings |
| `(string-length s)` | Length in bytes |
| `(substring s start end)` | Substring by byte index |
| `(string-index hay needle)` | Index of first occurrence, or `#f` |
| `(string-upcase s)` | Uppercase |
| `(string-downcase s)` | Lowercase |
| `(number->string n)` | Number to decimal string |
| `(string->number s)` | Parse number; `#f` on failure |

The following string utilities are implemented in the **stdlib** rather than
as Rust primitives, as they can be expressed using the core functions above:
`string-contains`, `string-split`, `string-join`, `string-lines`,
`string-unlines`, `string-trim`.

### I/O and control

| Function | Description |
|---|---|
| `(display v…)` | Write to terminal (display form) |
| `(write v…)` | Write to terminal (write/repr form) |
| `(newline)` | No-op |
| `(error msg…)` | Raise a runtime error |
| `(assert v)` | Error if `v` is falsy |

### Loading scripts

| Function | Description |
|---|---|
| `(include path)` | Evaluate all forms in `path.content` into the session environment |

---

## 6. Pipe threading

Inside a `(…)` expression, `|` is a threading operator. The accumulated value
is passed as the **first argument** of the next stage (thread-first). An
explicit `_` placeholder overrides placement.

```scheme
; Thread-first (default):
(@sky#room:who | (search-by "hans") | length)
; → (length (search-by who-list "hans"))

; Explicit _ placeholder:
(@sky#room:who | (take _ 5) | (join _ "\n"))
; → (join (take who-list 5) "\n")

; Multi-stage:
(@sky#room:inventory | string-lines | (take 10) | length)
```

`|` with a function that has no extra args simply applies the function:

```scheme
(@sky#room:who | string-lines | length)  ; count lines in reply
```

**Pipe vs. side-effects**: `!verb` system operations cannot be piped.
Pipe only works on values produced by Scheme expressions and RPC calls.

---

## 7. Send primitives

These functions provide explicit control over ma message sending and return
structured reply tuples rather than auto-unwrapped strings.

### `rpc-send`

```scheme
(rpc-send target verb arg…) → (:ok value) | (:error reason) | (:timeout)
```

Sends an RPC and **blocks** (cooperatively) until the reply arrives.

- `target` — `"@alias#fragment"` or `"did:ma:…#fragment"`. Alias resolution
  is performed automatically.
- `verb` — `":enter"`, `":ping"` etc. The leading `:` is optional.
- `arg…` — zero or more additional string arguments.

### `msg-send`

```scheme
(msg-send target body) → (:ok msg-id) | (:error reason)
```

Sends a plain-text inbox message. Returns immediately after dispatch.

### `chat-send`

```scheme
(chat-send target text) → (:ok msg-id) | (:error reason)
```

Sends an ephemeral chat message.

### `emote-send`

```scheme
(emote-send target text) → (:ok msg-id) | (:error reason)
```

Sends an emote message.

---

## 8. Reply tuple helpers

Reply tuples are lists whose first element is a keyword string: `":ok"`,
`":error"`, or `":timeout"`.

| Function | Description |
|---|---|
| `(ok? reply)` | True if `(car reply)` is `":ok"` |
| `(err? reply)` | True if `(car reply)` is `":error"` |
| `(ok-val reply)` | Second element of `(:ok value)` |
| `(err-msg reply)` | Second element of `(:error reason)` |

---

## 9. Session environment

Definitions made with `(define …)` persist in a **session environment** for
the duration of the login session:

- Initialised at login.
- Cleared on logout.
- Stored in WASM memory — does **not** survive a page refresh.

To persist values across sessions, write to config:

```scheme
(.my.config.counter: (number->string (+ 1 (string->number (.my.config.counter)))))
```

### Scripting with `.my.doc`

Scripts may be stored in any config path with a `.content` subkey and
evaluated with `:eval`:

```
.my.doc.boot.ma:edit      ; write in CodeMirror (syntax-highlighted for .ma)
.my.doc.boot.ma:eval      ; evaluate into session environment
.my.doc.boot.ma:publish @ma  ; publish to IPFS
```

Path names ending in `.ma` open in CodeMirror with Scheme syntax highlighting.

---

## 10. Limitations

### No tail-call optimisation

The evaluator uses recursive `async fn` calls. Deep tail-recursive loops
(> ~1 000 frames) may exhaust the WASM async stack. Use `fold` or iterative
patterns for large accumulations.

### Scheme in sync batches

Scheme expansion is asynchronous (`spawn_local`). Inside a `.batch:sync`
block, a Scheme-containing line does not block the batch step counter — the
expanded line re-queues and may arrive after the batch has already advanced.
Avoid Scheme expressions inside sync batches.

### System operations (`!verb`) not callable from Scheme

`.path!verb` forms (`!edit`, `!eval`, `!save`, `!publish`, etc.) are
side-effect operations requiring the full terminal dispatch context. They
cannot be called from within `(…)` Scheme expressions. Use them from the
normal command line.

Dot-path config operations (`Get`, `Set`, `Delete`) remain available from
Scheme via the `.path` MaPath form.
