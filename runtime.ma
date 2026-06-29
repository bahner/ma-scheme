; zscheme runtime.ma v0.1
;
; High-level helpers for connecting to a ma runtime.
; Load stdlib first, then this file:
;
;   (include ".my.scheme.stdlib.ma")
;   (include ".my.scheme.runtime.ma")
;
; Save to .my.scheme.runtime.ma:
;   .my.scheme.runtime.ma!edit
;
; Publish:   .my.scheme.runtime.ma!publish @ma
; Fetch:     .my.scheme.runtime.ma!fetch <cid>

; ── Address parsing ───────────────────────────────────────────────────────────
;
; Addresses follow the form:  [avatar@]runtime[#room]
;
;   "alice@sky#room"  →  ("alice"  "@sky"  "#room")
;   "@sky#room"       →  (#f       "@sky"  "#room")
;   "@sky"            →  (#f       "@sky"  #f)
;
; runtime is always returned with its @ sigil.
; room    is always returned with its # sigil, or #f when absent.

(define ENTER  ":enter")
(define CLAIM  ":claim")
(define AVATAR "#avatar")
(define HOUSE  "#house")

(define (parse-address addr)
  (let* ((at-idx  (string-index addr "@"))
         (hs-idx  (string-index addr "#"))
         (len     (string-length addr))
         (avatar  (if (and at-idx (> at-idx 0))
                      (substring addr 0 at-idx)
                      #f))
         (rt-start (if at-idx (+ at-idx 1) 0))
         (rt-end   (if hs-idx hs-idx len))
         (runtime  (string-append "@" (substring addr rt-start rt-end)))
         (room     (if hs-idx (substring addr hs-idx len) #f)))
    (list avatar runtime room)))

(define (address-avatar  p) (car   p))
(define (address-runtime p) (cadr  p))
(define (address-room    p) (caddr p))

; ── Individual steps ──────────────────────────────────────────────────────────

; Claim (or update) your display name on a runtime.
; (claim-alias "@sky" "alice")  →  avatar-id string
(define (claim-alias runtime name)
  (let ((r (rpc-send (string-append runtime AVATAR) CLAIM name)))
    (if (ok? r)
        (ok-val r)
        (error (string-append "claim-alias: " (err-msg r))))))

; Obtain an entry ticket from the runtime's house entity.
; room must include the # sigil, e.g. "#room".
; (get-room-ticket "@sky" "#room")  →  ticket string
(define (get-room-ticket runtime room)
  (let ((r (rpc-send (string-append runtime HOUSE) ENTER room)))
    (if (ok? r)
        (ok-val r)
        (error (string-append "get-room-ticket: " (err-msg r))))))

; Present a ticket to the room and complete the entry handshake.
; (enter-room "@sky" "#room" ticket)  →  room's welcome reply
(define (enter-room runtime room ticket)
  (let ((r (rpc-send (string-append runtime room) ENTER ticket)))
    (if (ok? r)
        (ok-val r)
        (error (string-append "enter-room: " (err-msg r))))))

; ── Composite ─────────────────────────────────────────────────────────────────
;
; (enter "alice@sky#room")  — claim avatar name, get ticket, enter room.
; (enter "@sky#room")       — skip avatar claim, get ticket, enter room.
;
; Returns the room's welcome reply.
(define (enter addr)
  (let* ((p       (parse-address addr))
         (avatar  (address-avatar  p))
         (runtime (address-runtime p))
         (room    (address-room    p)))
    (when (equal? runtime "@")
      (error (string-append "enter: no runtime in address: " addr)))
    (unless room
      (error (string-append "enter: no room in address: " addr)))
    (when avatar
      (claim-alias runtime avatar))
    (let ((ticket (get-room-ticket runtime room)))
      (enter-room runtime room ticket))))
