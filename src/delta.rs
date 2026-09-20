use crate::exec::StepResult;
use crate::verify::AssertResult;

/// Compact packet sent back to the model — never the full transcript.
#[derive(Debug, Clone)]
pub struct Delta {
    pub ok: bool,
    pub summary: String,
}

pub fn pack(steps: &[StepResult], asserts: &[AssertResult]) -> Delta {
    let ok = asserts.iter().all(|a| a.ok);
    let mut lines = Vec::new();
    lines.push(format!("OK={}", ok));
    for s in steps {
        let mark = if s.ok { "✓" } else { "✗" };
        lines.push(format!("{mark} {} (exit {} · {}ms)", s.title, s.exit, s.ms));
        if !s.ok {
            let tail = s.output.lines().rev().take(12).collect::<Vec<_>>();
            for t in tail.into_iter().rev() {
                lines.push(format!("    {t}"));
            }
        }
    }
    lines.push("ASSERTS:".into());
    for a in asserts {
        let mark = if a.ok { "✓" } else { "✗" };
        lines.push(format!("{mark} {} — {}", a.label, a.detail));
    }
    if !ok {
        lines.push("NEXT: emit a smaller BURST that repairs the failing assert only.".into());
    }
    Delta {
        ok,
        summary: lines.join("\n"),
    }
}
