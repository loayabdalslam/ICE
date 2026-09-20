use crate::exec::StepResult;
use crate::ir::Assert;
use crate::sandbox::resolve;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct AssertResult {
    pub label: String,
    pub ok: bool,
    pub detail: String,
}

pub fn check(root: &Path, asserts: &[Assert], steps: &[StepResult]) -> Vec<AssertResult> {
    let last_exit = steps.last().map(|s| s.exit).unwrap_or(0);
    asserts
        .iter()
        .map(|a| match a {
            Assert::ExitZero => AssertResult {
                label: "exit 0".into(),
                ok: last_exit == 0,
                detail: format!("last exit={last_exit}"),
            },
            Assert::ExitCode(c) => AssertResult {
                label: format!("exit {c}"),
                ok: last_exit == *c,
                detail: format!("last exit={last_exit}"),
            },
            Assert::Contains { path, text } => file_has(root, path, text, true),
            Assert::NotContains { path, text } => file_has(root, path, text, false),
            Assert::FileExists { path } => {
                let p = resolve(root, path).ok();
                let ok = p.as_ref().map(|x| x.exists()).unwrap_or(false);
                AssertResult {
                    label: format!("exists {path}"),
                    ok,
                    detail: if ok { "found".into() } else { "missing".into() },
                }
            }
            Assert::FileNotExists { path } => {
                let p = resolve(root, path).ok();
                let exists = p.as_ref().map(|x| x.exists()).unwrap_or(false);
                AssertResult {
                    label: format!("not_exists {path}"),
                    ok: !exists,
                    detail: if exists {
                        "still present".into()
                    } else {
                        "absent".into()
                    },
                }
            }
        })
        .collect()
}

fn file_has(root: &Path, path: &str, text: &str, want: bool) -> AssertResult {
    match resolve(root, path).and_then(|p| std::fs::read_to_string(p).map_err(|e| e.into())) {
        Ok(body) => {
            let has = body.contains(text);
            AssertResult {
                label: format!(
                    "{} {path} {text}",
                    if want { "contains" } else { "not_contains" }
                ),
                ok: has == want,
                detail: if has {
                    "match".into()
                } else {
                    "no match".into()
                },
            }
        }
        Err(e) => AssertResult {
            label: format!("read {path}"),
            ok: false,
            detail: e.to_string(),
        },
    }
}

pub fn all_ok(rs: &[AssertResult]) -> bool {
    !rs.is_empty() && rs.iter().all(|r| r.ok)
}
