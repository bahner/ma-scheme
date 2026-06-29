/// File executor: read a script file, strip shebang, evaluate all expressions.

use std::path::Path;

use anyhow::{Context, Result};

use crate::context::Ctx;
use crate::scheme::eval_source;

/// Execute a zscheme script file.
///
/// - Strips a shebang line (`#!`) if present on the first line.
/// - Evaluates all top-level Scheme expressions in order.
/// - Returns `Ok(())` on success; prints errors to stderr.
pub async fn run_file(path: &Path, ctx: Ctx) -> Result<()> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read script: {}", path.display()))?;

    let source = strip_shebang(&raw);

    match eval_source(source, ctx).await {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("error: {e}");
            Err(anyhow::anyhow!("{e}"))
        }
    }
}

/// Strip a leading shebang line (`#!…`) if present.
fn strip_shebang(source: &str) -> &str {
    if source.starts_with("#!") {
        match source.find('\n') {
            Some(pos) => &source[pos + 1..],
            None => "",
        }
    } else {
        source
    }
}

#[cfg(test)]
mod tests {
    use super::strip_shebang;

    #[test]
    fn strips_shebang() {
        assert_eq!(strip_shebang("#!/usr/local/bin/zscheme\n(+ 1 2)"), "(+ 1 2)");
    }

    #[test]
    fn no_shebang_unchanged() {
        assert_eq!(strip_shebang("(+ 1 2)"), "(+ 1 2)");
    }
}
