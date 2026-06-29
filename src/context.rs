/// Evaluation context for the zscheme CLI interpreter.
///
/// `CliCtx` replaces `EvalCtx { AppState, RwSignal<EgoConfig> }` from ma-agent.
/// It is wrapped in `Rc` for cheap clone across recursive eval calls.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::channel::oneshot;
use ma_core::{Did, IpfsGatewayResolver, Message, SigningKey, RPC_PROTOCOL_ID};

use crate::config::{parse_actor_command, parse_dot_command, DotOp, SchemeConfig};
use crate::scheme::{
    eval::SchemeErr,
    get_env,
    parser::{parse_expr, tokenize},
    value::SchemeVal,
};

// ── Context ────────────────────────────────────────────────────────────────

/// Shared evaluation context threaded through all recursive eval calls.
pub struct CliCtx {
    /// Flat key-value configuration (aliases, config values, etc.)
    pub config: RefCell<SchemeConfig>,
    /// Our own DID (e.g. `did:ma:abc`)
    pub our_did: String,
    /// Ed25519 signing key bytes for outgoing messages.
    pub signing_key_bytes: [u8; 32],
    /// iroh endpoint for sending/receiving messages.
    pub endpoint: RefCell<Box<dyn ma_core::MaEndpoint>>,
    /// DID resolver for looking up actor endpoints.
    pub resolver: Rc<IpfsGatewayResolver>,
    /// Pending RPC reply senders keyed by message id.
    pub reply_senders: RefCell<HashMap<String, oneshot::Sender<Result<String, String>>>>,
    /// RPC inbox for receiving replies.
    pub rpc_inbox: RefCell<ma_core::Inbox<Message>>,
    /// Kubo RPC base URL (e.g. `http://127.0.0.1:5001`).
    pub kubo_rpc_url: String,
    /// IPFS gateway fallback URL (e.g. `https://dweb.link`).
    pub gateway_url: String,
    /// reqwest client (reused across CID fetches).
    pub http: reqwest::Client,
}

/// Reference-counted context — cheap to clone.
pub type Ctx = Rc<CliCtx>;

// ── Constructor ────────────────────────────────────────────────────────────

impl CliCtx {
    pub fn new(
        config: SchemeConfig,
        our_did: String,
        signing_key_bytes: [u8; 32],
        endpoint: Box<dyn ma_core::MaEndpoint>,
        resolver: Rc<IpfsGatewayResolver>,
        rpc_inbox: ma_core::Inbox<Message>,
        kubo_rpc_url: String,
        gateway_url: String,
    ) -> Ctx {
        Rc::new(Self {
            config: RefCell::new(config),
            our_did,
            signing_key_bytes,
            endpoint: RefCell::new(endpoint),
            resolver,
            reply_senders: RefCell::new(HashMap::new()),
            rpc_inbox: RefCell::new(rpc_inbox),
            kubo_rpc_url,
            gateway_url,
            http: reqwest::Client::new(),
        })
    }
}

// ── Config access ──────────────────────────────────────────────────────────

impl CliCtx {
    /// Get a config value at `path`.
    pub fn config_get(&self, path: &str) -> Option<String> {
        self.config.borrow().get_str(path)
    }

    /// Set a config value.
    #[allow(dead_code)]
    pub fn config_set(&self, path: &str, value: &str) {
        self.config.borrow_mut().set(path, value);
    }

    /// Delete a config subtree.
    #[allow(dead_code)]
    pub fn config_delete(&self, path: &str) {
        self.config.borrow_mut().delete_subtree(path);
    }

    /// Resolve an actor target (alias or DID) to its full form.
    pub fn resolve_target(&self, raw: &str) -> Result<String, String> {
        self.config.borrow().resolve_target(raw)
    }
}

// ── Output ─────────────────────────────────────────────────────────────────

impl CliCtx {
    /// Print output to stdout (replaces `AppState::push_system`).
    pub fn display_output(&self, text: &str) {
        print!("{text}");
    }
}

// ── CID fetch ──────────────────────────────────────────────────────────────

impl CliCtx {
    /// Fetch the text content of an IPFS CID.
    /// Tries local Kubo first, falls back to the IPFS gateway.
    pub async fn fetch_cid(&self, cid: &str) -> Result<String, String> {
        // Try local Kubo: POST /api/v0/cat?arg=<cid>
        let kubo_url = format!("{}/api/v0/cat?arg={}", self.kubo_rpc_url.trim_end_matches('/'), cid);
        match self.http.post(&kubo_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                return resp.text().await.map_err(|e| e.to_string());
            }
            _ => {}
        }
        // Fall back to gateway
        let gw_url = format!("{}/ipfs/{}", self.gateway_url.trim_end_matches('/'), cid);
        self.http
            .get(&gw_url)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .text()
            .await
            .map_err(|e| e.to_string())
    }
}

// ── RPC transport ──────────────────────────────────────────────────────────

impl CliCtx {
    /// Send an RPC message and return the message id.
    pub async fn send_rpc(
        &self,
        target: &str,
        verb: &str,
        args: &[String],
    ) -> Result<String, String> {
        let did = Did::try_from(target).map_err(|e| e.to_string())?;
        let signing_key = SigningKey::from_private_key_bytes(did, self.signing_key_bytes)
            .map_err(|e| e.to_string())?;

        let atom = if verb.starts_with(':') {
            verb.to_string()
        } else {
            format!(":{verb}")
        };

        let cbor_val = if args.is_empty() {
            ciborium::Value::Text(atom)
        } else {
            let mut items = Vec::with_capacity(1 + args.len());
            items.push(ciborium::Value::Text(atom));
            for a in args {
                items.push(ciborium::Value::Text(a.clone()));
            }
            ciborium::Value::Array(items)
        };

        let mut body = Vec::new();
        ciborium::ser::into_writer(&cbor_val, &mut body).map_err(|e| e.to_string())?;

        let msg = ma_core::Message::new(
            &self.our_did,
            target,
            ma_core::MESSAGE_TYPE_RPC,
            ma_core::CONTENT_TYPE_TERM,
            &body,
            &signing_key,
        )
        .map_err(|e| e.to_string())?;

        let msg_id = msg.id.clone();
        let mut outbox = self
            .endpoint
            .borrow()
            .outbox(self.resolver.as_ref(), target, RPC_PROTOCOL_ID)
            .await
            .map_err(|e| e.to_string())?;
        outbox.send(&msg).await.map_err(|e| e.to_string())?;
        Ok(msg_id)
    }

    /// Send a plain-text inbox message (fire and forget).
    pub async fn send_text(&self, target: &str, text: &str) -> Result<String, String> {
        use ma_core::{INBOX_PROTOCOL_ID, MESSAGE_TYPE_MESSAGE};
        let did = Did::try_from(target).map_err(|e| e.to_string())?;
        let signing_key = SigningKey::from_private_key_bytes(did, self.signing_key_bytes)
            .map_err(|e| e.to_string())?;
        let msg = ma_core::Message::new(
            &self.our_did,
            target,
            MESSAGE_TYPE_MESSAGE,
            "text/plain",
            text.as_bytes(),
            &signing_key,
        )
        .map_err(|e| e.to_string())?;
        let msg_id = msg.id.clone();
        let mut outbox = self
            .endpoint
            .borrow()
            .outbox(self.resolver.as_ref(), target, INBOX_PROTOCOL_ID)
            .await
            .map_err(|e| e.to_string())?;
        outbox.send(&msg).await.map_err(|e| e.to_string())?;
        Ok(msg_id)
    }

    /// Close the iroh endpoint gracefully.
    pub async fn close(&self) {
        self.endpoint.borrow_mut().close().await;
    }

    /// Register a oneshot sender to await the reply for `msg_id`.
    pub fn register_reply_sender(
        &self,
        msg_id: String,
        sender: oneshot::Sender<Result<String, String>>,
    ) {
        self.reply_senders.borrow_mut().insert(msg_id, sender);
    }

    /// Drain RPC inbox and route replies to waiting senders.
    /// Call periodically from a poll loop.
    pub fn poll_rpc_replies(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let messages: Vec<Message> = self.rpc_inbox.borrow_mut().drain(now);

        for msg in messages {
            let reply_to = match &msg.reply_to {
                Some(id) => id.clone(),
                None => continue, // unsolicited — ignore for now
            };

            // Decode CBOR payload — error/ok determined by content, not message type.
            let payload = msg.payload();
            let reply_result = decode_rpc_reply(&payload);

            if let Some(sender) = self.reply_senders.borrow_mut().remove(&reply_to) {
                let _ = sender.send(reply_result);
            }
        }
    }
}

// ── Dot-path evaluation ────────────────────────────────────────────────────

impl CliCtx {
    /// Evaluate a dot-path command and return the result as a SchemeVal.
    pub fn eval_dot(&self, command: &str) -> Result<SchemeVal, SchemeErr> {
        let (path, op) =
            parse_dot_command(command).ok_or_else(|| SchemeErr::MaError(format!("bad dot command: {command}")))?;

        match op {
            DotOp::Get => {
                if let Some(val) = self.config.borrow().get_str(&path) {
                    Ok(SchemeVal::Str(val))
                } else {
                    let pairs = self.config.borrow().list(&path);
                    if pairs.is_empty() {
                        Err(SchemeErr::MaError(format!("no value at .{path}")))
                    } else {
                        Ok(SchemeVal::List(
                            pairs
                                .into_iter()
                                .map(|(k, _)| SchemeVal::Str(k))
                                .collect(),
                        ))
                    }
                }
            }
            DotOp::Set(val) => {
                self.config.borrow_mut().set(&path, &val);
                Ok(SchemeVal::Nil)
            }
            DotOp::Delete => {
                self.config.borrow_mut().delete_subtree(&path);
                Ok(SchemeVal::Nil)
            }
            DotOp::Meta { verb, args } => {
                // Verb dispatch not yet fully supported in CLI — print a warning.
                tracing::warn!(path = %path, verb = %verb, args = %args,
                    "dot-path verb dispatch not supported in CLI scripts");
                Ok(SchemeVal::Nil)
            }
        }
    }
}

// ── Actor evaluation ───────────────────────────────────────────────────────

impl CliCtx {
    /// Send an actor RPC and await the reply. Returns the reply as a SchemeVal.
    pub async fn eval_actor(self: &Rc<Self>, command: &str) -> Result<SchemeVal, SchemeErr> {
        let effective = if command.starts_with('@') || command.starts_with("did:") {
            command.to_string()
        } else {
            format!("@{command}")
        };

        let cfg = self.config.borrow().clone();
        let (target, verb, args) =
            parse_actor_command(&effective, &cfg).map_err(SchemeErr::MaError)?;

        let msg_id = self
            .send_rpc(&target, &verb, &args)
            .await
            .map_err(SchemeErr::MaError)?;

        let (sender, receiver) = oneshot::channel::<Result<String, String>>();
        self.register_reply_sender(msg_id, sender);

        match receiver.await {
            Ok(Ok(content)) => Ok(SchemeVal::Str(content)),
            Ok(Err(e)) => Err(SchemeErr::MaError(e)),
            Err(_) => Err(SchemeErr::MaError(
                "RPC reply channel cancelled (disconnected?)".to_string(),
            )),
        }
    }
}

// ── Include ────────────────────────────────────────────────────────────────

impl CliCtx {
    /// Evaluate a source string in the session environment (for `include`).
    pub async fn eval_include(self: &Rc<Self>, source: &str) -> Result<SchemeVal, SchemeErr> {
        use crate::scheme::eval::eval;
        let env = get_env();
        let tokens = tokenize(source).map_err(|e| SchemeErr::ParseError(e.to_string()))?;
        let mut pos = 0;
        let mut last = SchemeVal::Nil;
        while pos < tokens.len() {
            let (expr, next) =
                parse_expr(&tokens, pos).map_err(|e| SchemeErr::ParseError(e.to_string()))?;
            last = eval(expr, env.clone(), Rc::clone(self)).await?;
            pos = next;
        }
        Ok(last)
    }
}

// ── CBOR reply decoding ────────────────────────────────────────────────────

fn decode_rpc_reply(payload: &[u8]) -> Result<String, String> {
    use ciborium::Value as V;
    match ciborium::de::from_reader::<V, _>(payload) {
        Ok(V::Text(s)) if s == ":ok" => Ok(String::new()),
        Ok(V::Text(s)) => Ok(s),
        Ok(V::Array(items)) => match (items.first(), items.get(1)) {
            (Some(V::Text(verb)), value) if verb == ":ok" => match value {
                Some(V::Text(s)) => Ok(s.clone()),
                None => Ok(String::new()),
                _ => Ok(String::from_utf8_lossy(payload).to_string()),
            },
            (Some(V::Text(verb)), value) if verb == ":error" => {
                let reason = value
                    .and_then(|v| if let V::Text(s) = v { Some(s.clone()) } else { None })
                    .unwrap_or_else(|| String::from_utf8_lossy(payload).to_string());
                Err(reason)
            }
            _ => Ok(String::from_utf8_lossy(payload).to_string()),
        },
        _ => Ok(String::from_utf8_lossy(payload).to_string()),
    }
}
