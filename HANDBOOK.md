# zscheme Handbook

Practical guide to scripting the 間 actor platform with zscheme.

---

## Table of contents

1. [First steps](#1-first-steps)
2. [Working with config](#2-working-with-config)
3. [Calling actors](#3-calling-actors)
4. [Loading the stdlib](#4-loading-the-stdlib)
5. [Writing scripts](#5-writing-scripts)
6. [World navigation](#6-world-navigation)
7. [Error handling](#7-error-handling)
8. [Tips and patterns](#8-tips-and-patterns)

---

## 1. First steps

Open `zion` and type a `(…)` expression:

```scheme
(+ 2 3)               ; → 5
(string-append "hello" " world")  ; → hello world
```

The result appears in the terminal output. Definitions persist for the
session:

```scheme
(define greeting "hello")
(string-append greeting " world")  ; → hello world
```

Redefine at any time:

```scheme
(define greeting "greetings")
```

---

## 2. Working with config

Read a value:

```scheme
(.my.aliases.sky)             ; → did:ma:…
(.my.config.colour.text)      ; → #00ff41
```

Write a value:

```scheme
(.my.config.greeting: "hello")
```

Delete a subtree:

```scheme
(.my.temp:)
```

Compose paths dynamically:

```scheme
(define (alias name)
  (string-append ".my.aliases." name))

; Now use it:
((alias "sky"))               ; → resolves .my.aliases.sky
```

---

## 3. Calling actors

### The `@` shorthand

The simplest form — sends an RPC and returns the reply value directly.
Raises an error on failure.

```scheme
(@sky#ping)                       ; → ":pong"
(@sky#house:enter #room)          ; → "ticket-xyz"
```

### DID in function position

When a config lookup returns a DID, you can use it as a function:

```scheme
(define sky (.my.aliases.sky))
(sky "#house:enter" "#room")      ; sends to did:ma:…#house:enter
```

### `rpc-send` for explicit error handling

```scheme
(define result (rpc-send "@sky#house" ":enter" "#room"))
(if (ok? result)
    (display (string-append "ticket: " (ok-val result)))
    (error (err-msg result)))
```

---

## 4. Loading the stdlib

The stdlib provides `string-split`, `string-join`, `map`, `filter`, `fold`,
`append`, `reverse`, `length`, and more.

**Fetch once** (saves to local config):

```
.my.doc.stdlib.ma:fetch <cid>
```

**Load into session** (run after each login, or put in a boot script):

```scheme
(include ".my.doc.stdlib.ma")
```

After loading:

```scheme
(map (lambda (x) (* x x)) '(1 2 3 4 5))  ; → (1 4 9 16 25)
(filter odd? '(1 2 3 4 5))               ; → (1 3 5)
(string-split "a,b,c" ",")               ; → ("a" "b" "c")
```

---

## 5. Writing scripts

Scripts are stored in any config path with a `.content` subkey. The `.ma`
extension is conventional and enables Scheme syntax highlighting in the editor.

**Create or edit:**

```
.my.doc.greet.ma:edit
```

Paste your script, then click **Save**.

**Evaluate** (loads all `define` forms into session env):

```
.my.doc.greet.ma:eval
```

**Call a function from the script:**

```
.my.doc.greet.ma:greet "alice"
```

This is equivalent to: evaluate the script, then call `(greet "alice")`.

**Share via IPFS:**

```
.my.doc.greet.ma:publish @ma
.my.doc.greet.ma:cid
```

Others load it with:

```
.my.doc.greet.ma:fetch <cid>
.my.doc.greet.ma:eval
```

---

## 6. World navigation

The address format `alias@runtime#room` encodes an avatar alias, a runtime
alias, and a room fragment in a single string.

Store a navigation script (see `stdlib.ma` for `string-index`, `string-split`):

```scheme
; .my.doc.world.ma — save with :edit, load with :eval

(define (parse-addr addr)
  (let* ((at      (string-index addr "@"))
         (hash    (string-index addr "#"))
         (alias   (substring addr 0 at))
         (runtime (string-append "@" (substring addr (+ at 1) hash)))
         (room    (substring addr hash (string-length addr))))
    (list alias runtime room)))

(define (enter addr)
  (let* ((parts   (parse-addr addr))
         (alias   (car   parts))
         (runtime (cadr  parts))   ; requires stdlib
         (room    (caddr parts))   ; requires stdlib
         (target  (string-append runtime room))
         (_       (rpc-send (string-append runtime "#avatar") ":claim" alias))
         (result  (rpc-send (string-append runtime "#house") ":enter" room)))
    (if (ok? result)
        (let ((entered (rpc-send target ":enter" (ok-val result))))
          (if (ok? entered)
              (begin (use target) (ok-val entered))
              (error (err-msg entered))))
        (error (err-msg result)))))
```

Call it:

```
.my.doc.world.ma:enter "alice@sky#room"
```

Or share as a URL:

```
https://zion.bahner.com/?ctx=@sky#room
```

---

## 7. Error handling

The `@` shorthand raises on error (suitable for interactive use):

```scheme
(@sky#ping)   ; raises SchemeErr if :ping fails
```

Use `rpc-send` when you need to handle errors gracefully:

```scheme
(define (safe-ping target)
  (let ((r (rpc-send target ":ping")))
    (if (ok? r) "online" "offline")))

(safe-ping "@sky")   ; → "online" or "offline"
```

Raise your own errors:

```scheme
(define (require-value v msg)
  (if (equal? v #f) (error msg) v))

(require-value (.my.aliases.sky) "sky alias not set")
```

---

## 8. Tips and patterns

### Boot script

Create `.my.doc.boot.ma` with your session initialisations and call:

```scheme
(include ".my.doc.boot.ma")
```

Or add to startup config — if `.my.ctx.use` is `"true"` at login, focus is
restored automatically.

### Named constants

```scheme
(define ROOM   "#room")
(define HOUSE  "#house")
(define SKY    (.my.aliases.sky))

(rpc-send (string-append SKY HOUSE) ":enter" ROOM)
```

### Combining results

```scheme
(define sky (.my.aliases.sky))
(define ticket (ok-val (rpc-send (string-append sky "#house") ":enter" "#room")))
(rpc-send (string-append sky "#room") ":enter" ticket)
```

### URL sharing

Any `?ctx=` parameter is applied automatically after login:

```
https://zion.bahner.com/?ctx=@sky#room
```

Combine with `?say=` to pre-fill the input field:

```
https://zion.bahner.com/?ctx=@sky#room&say=sky#room
```
