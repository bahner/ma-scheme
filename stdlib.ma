; zscheme stdlib v0.1
;
; Userspace implementations of common functions.
; All of these can be implemented using the core Rust primitives
; (string-index, substring, string-length, string-append, cons,
;  car, cdr, null?, pair?, =, <, >, mod, not, display, error).
;
; Load with: (include ".my.scheme.stdlib.ma")
; Or from terminal: .my.scheme.stdlib.ma!eval
;
; Publish to IPFS: .my.scheme.stdlib.ma!publish @ma
; Then share the CID so others can: .my.scheme.stdlib.ma!fetch <cid>

; ── List aliases ──────────────────────────────────────────────────────────────

(define (cadr x)   (car (cdr x)))
(define (caddr x)  (car (cdr (cdr x))))
(define (cadddr x) (car (cdr (cdr (cdr x)))))

; ── Type predicates ───────────────────────────────────────────────────────────

(define (list? x) (or (null? x) (pair? x)))

; ── Aliases ───────────────────────────────────────────────────────────────────

(define eq?       equal?)
(define eqv?      equal?)
(define quotient  /)
(define remainder mod)

; ── List operations ───────────────────────────────────────────────────────────

(define (length lst)
  (let loop ((l lst) (n 0))
    (if (null? l) n (loop (cdr l) (+ n 1)))))

(define (list-ref lst i)
  (if (= i 0)
      (car lst)
      (list-ref (cdr lst) (- i 1))))

(define (append a b)
  (if (null? a)
      b
      (cons (car a) (append (cdr a) b))))

(define (reverse lst)
  (let loop ((l lst) (acc '()))
    (if (null? l) acc (loop (cdr l) (cons (car l) acc)))))

(define (map f lst)
  (if (null? lst)
      '()
      (cons (f (car lst)) (map f (cdr lst)))))

(define (filter f lst)
  (cond ((null? lst)       '())
        ((f (car lst))     (cons (car lst) (filter f (cdr lst))))
        (else              (filter f (cdr lst)))))

(define (for-each f lst)
  (unless (null? lst)
    (f (car lst))
    (for-each f (cdr lst))))

(define (fold f init lst)
  (if (null? lst)
      init
      (fold f (f (car lst) init) (cdr lst))))

(define fold-left fold)

(define (any f lst)
  (cond ((null? lst) #f)
        ((f (car lst)) #t)
        (else (any f (cdr lst)))))

(define (every f lst)
  (cond ((null? lst) #t)
        ((not (f (car lst))) #f)
        (else (every f (cdr lst)))))

; ── Numeric utilities ─────────────────────────────────────────────────────────

(define (abs n)       (if (< n 0) (- n) n))
(define (max a b)     (if (> a b) a b))
(define (min a b)     (if (< a b) a b))
(define (zero? n)     (= n 0))
(define (positive? n) (> n 0))
(define (negative? n) (< n 0))
(define (even? n)     (= (mod n 2) 0))
(define (odd? n)      (not (even? n)))

; ── String utilities ──────────────────────────────────────────────────────────

(define (string-contains hay needle)
  (not (equal? (string-index hay needle) #f)))

(define (string-split s delim)
  (let ((dlen (string-length delim)))
    (let loop ((rest s) (acc '()))
      (let ((idx (string-index rest delim)))
        (if (equal? idx #f)
            (reverse (cons rest acc))
            (loop (substring rest (+ idx dlen) (string-length rest))
                  (cons (substring rest 0 idx) acc)))))))

(define (string-join lst delim)
  (if (null? lst)
      ""
      (let loop ((rest (cdr lst)) (acc (car lst)))
        (if (null? rest)
            acc
            (loop (cdr rest)
                  (string-append acc delim (car rest)))))))

(define (string-lines s)   (string-split s "\n"))
(define (string-unlines l) (string-join l "\n"))

(define (string-trim s)
  (let* ((len (string-length s))
         (start (let loop ((i 0))
                  (if (or (= i len) (not (string-contains " \t\n\r" (substring s i (+ i 1)))))
                      i
                      (loop (+ i 1)))))
         (end   (let loop ((i (- len 1)))
                  (if (or (< i start) (not (string-contains " \t\n\r" (substring s i (+ i 1)))))
                      (+ i 1)
                      (loop (- i 1))))))
    (substring s start end)))

(define (take n lst)
  (if (or (null? lst) (= n 0))
      '()
      (cons (car lst) (take (- n 1) (cdr lst)))))

(define (drop n lst)
  (if (or (null? lst) (= n 0))
      lst
      (drop (- n 1) (cdr lst))))

(define (member item lst)
  (cond ((null? lst) #f)
        ((equal? item (car lst)) lst)
        (else (member item (cdr lst)))))

(define (contains? lst item)
  (not (equal? (member item lst) #f)))

; View helpers — replaces Rust :cat :head :tail :wc
(define (doc-cat path)
  (eval (string-append "(" path ".content)")))

(define (wc-lines text) (length (string-lines text)))
(define (head-n n text) (string-unlines (take n (string-lines text))))
(define (tail-n n text)
  (let ((lines (string-lines text)))
    (string-unlines (drop (- (length lines) n) lines))))
