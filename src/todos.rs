use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Todo {
    pub id: u32,
    pub text: String,
    pub done: bool,
}

fn path(root: &Path) -> PathBuf {
    root.join(".ice/todos.json")
}

pub fn load(root: &Path) -> Vec<Todo> {
    let Ok(txt) = fs::read_to_string(path(root)) else {
        return Vec::new();
    };
    let v: serde_json::Value = serde_json::from_str(&txt).unwrap_or(json!([]));
    let arr = v.as_array().cloned().unwrap_or_default();
    arr.iter()
        .filter_map(|x| {
            Some(Todo {
                id: x.get("id")?.as_u64()? as u32,
                text: x.get("text")?.as_str()?.to_string(),
                done: x.get("done").and_then(|d| d.as_bool()).unwrap_or(false),
            })
        })
        .collect()
}

pub fn save(root: &Path, items: &[Todo]) -> anyhow::Result<()> {
    fs::create_dir_all(root.join(".ice"))?;
    let v: Vec<_> = items
        .iter()
        .map(|t| json!({"id": t.id, "text": t.text, "done": t.done}))
        .collect();
    fs::write(path(root), serde_json::to_string_pretty(&v)?)?;
    Ok(())
}

pub fn add(root: &Path, text: &str) -> Todo {
    let mut items = load(root);
    let id = items.iter().map(|t| t.id).max().unwrap_or(0) + 1;
    let t = Todo {
        id,
        text: text.to_string(),
        done: false,
    };
    items.push(t.clone());
    let _ = save(root, &items);
    t
}

pub fn complete(root: &Path, key: &str) -> Option<Todo> {
    let mut items = load(root);
    let mut found = None;
    for t in items.iter_mut() {
        if t.id.to_string() == key || t.text == key {
            t.done = true;
            found = Some(t.clone());
            break;
        }
    }
    let _ = save(root, &items);
    found
}

pub fn render(root: &Path) -> String {
    let items = load(root);
    if items.is_empty() {
        return "todo  (empty)".into();
    }
    let mut s = String::from("todo\n");
    for t in items {
        s.push_str(&format!(
            "  {} [{}] {}\n",
            if t.done { "✓" } else { "·" },
            t.id,
            t.text
        ));
    }
    s
}
